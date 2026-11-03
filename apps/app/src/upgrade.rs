//! Moving over from the Capacitor app (the Android build, from 1.0). The app this one installs over
//! seals its records and settings into a file only an app with the same id and signing key can
//! open (MigrationExport.kt, written by apps/web/src/lib/upgradeExport.ts). On the first start the
//! records go into the phone core's store before the core starts, and the settings into the app's
//! own; then the file goes. The secrets in it (the phone's key, hub tokens, server sign-ins) are
//! already where this app reads them: the same `inyon_secrets` file Kotlin's SshMethods keeps.

use std::path::Path;

use inyon_core::platform::store::Store;
use serde::Deserialize;
use serde_json::Value;

/// The export format this build reads (MigrationExport.PAYLOAD_VERSION).
const VERSION: u32 = 1;

/// Written once an import is committed, so a crash before the file is discarded doesn't import it
/// twice (the next start only discards it).
const DONE: &str = "upgrade-imported";

#[derive(Deserialize)]
struct Export {
    v: u32,
    #[serde(default)]
    records: serde_json::Map<String, Value>,
    #[serde(default)]
    local: serde_json::Map<String, Value>,
}

/// What an import brought over.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub records: usize,
    pub settings: usize,
    /// records this app already had, kept as they are
    pub kept: Vec<String>,
    /// records that weren't JSON, with why
    pub skipped: Vec<String>,
}

/// Why nothing was imported; the file stays, for a later build that can read it.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotJson(String),
    Version(u32),
}

/// Puts an export's records into `store` and its `inyon.*` settings through `save`. A record the
/// store already holds is kept: this app wrote it, so it's newer than the export.
pub fn import(payload: &str, store: &dyn Store, save: &mut dyn FnMut(&str, &str)) -> Result<Imported, Refused> {
    let e: Export = serde_json::from_str(payload).map_err(|e| Refused::NotJson(e.to_string()))?;
    if e.v != VERSION {
        return Err(Refused::Version(e.v));
    }
    let mut done = Imported::default();
    for (k, text) in &e.records {
        // the store keeps JSON text; anything else is a broken record
        let Some(v) = text.as_str().and_then(|t| serde_json::from_str::<Value>(t).ok()) else {
            done.skipped.push(format!("{k}: not JSON"));
            continue;
        };
        if store.get(k).is_some() {
            done.kept.push(k.clone());
            continue;
        }
        match store.set(k, &v) {
            Ok(()) => done.records += 1,
            Err(why) => done.skipped.push(format!("{k}: {why}")),
        }
    }
    for (k, v) in &e.local {
        if let (true, Some(v)) = (k.starts_with("inyon."), v.as_str()) {
            save(k, v);
            done.settings += 1;
        }
    }
    Ok(done)
}

/// The first start's import, before the core starts: `read` gives the opened file (or nothing),
/// `discard` deletes it and its key. `dir` is the app's folder; the core's records are in
/// `dir/core`.
pub fn first_start(dir: &Path, read: impl FnOnce() -> Option<String>, discard: impl FnOnce(), save: &mut dyn FnMut(&str, &str)) -> Option<Result<Imported, Refused>> {
    let marker = dir.join(DONE);
    if marker.exists() {
        // imported before, then stopped before the file went: only discard it now
        discard();
        return None;
    }
    let payload = read()?;
    let store = match inyon_backends::fs_store::FsStore::new(dir.join("core")) {
        Ok(s) => s,
        // the core couldn't keep records either; try again next start
        Err(e) => return Some(Err(Refused::NotJson(format!("the store didn't open: {e}")))),
    };
    let got = import(&payload, &store, save);
    if got.is_ok() && std::fs::write(&marker, b"").is_ok() {
        discard();
    }
    Some(got)
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
