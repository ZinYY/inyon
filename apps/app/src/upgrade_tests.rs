use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use inyon_core::platform::store::MemoryStore;
use serde_json::json;

use super::*;

/// What the Capacitor app seals (fixtures/upgrade/export-v1.json; the web app's test checks it
/// writes exactly this).
const EXPORT: &str = include_str!("../../../fixtures/upgrade/export-v1.json");

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("inyon-app-upgrade-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

type Saved = Rc<RefCell<BTreeMap<String, String>>>;

fn saver() -> (Saved, impl FnMut(&str, &str)) {
    let saved = Rc::new(RefCell::new(BTreeMap::new()));
    let s = saved.clone();
    (saved, move |k: &str, v: &str| drop(s.borrow_mut().insert(k.to_owned(), v.to_owned())))
}

#[test]
fn the_export_brings_the_records_and_the_settings_but_not_the_secrets() {
    let store = MemoryStore::new();
    let (saved, mut save) = saver();
    let got = import(EXPORT, &store, &mut save).unwrap();
    assert_eq!(got, Imported { records: 3, settings: 2, kept: vec![], skipped: vec![] });
    assert_eq!(store.get("hosts").unwrap()[0]["target"], json!("me@box"));
    assert_eq!(store.get("names").unwrap(), json!({ "box $1": "api" }));
    assert_eq!(saved.borrow()["inyon.keepAlive"], "0");
    assert_eq!(crate::notify::Wants::from_settings(saved.borrow().get("inyon.settings.v2").map(String::as_str)), crate::notify::Wants { done: false, pages: true });
    // the secrets are already in inyon_secrets, which this app reads too
    assert!(store.keys("").iter().all(|k| !k.contains("phone-key") && !k.starts_with("server:")));
    assert!(saved.borrow().keys().all(|k| k.starts_with("inyon.")));
}

#[test]
fn a_record_this_app_already_has_is_kept() {
    let store = MemoryStore::new();
    store.set("settings", &json!({ "autoResume": false })).unwrap();
    let got = import(EXPORT, &store, &mut |_, _| {}).unwrap();
    assert_eq!((got.records, got.kept), (2, vec!["settings".to_owned()]));
    assert_eq!(store.get("settings").unwrap()["autoResume"], json!(false));
}

#[test]
fn a_broken_record_is_left_out_and_the_rest_come_over() {
    let store = MemoryStore::new();
    let payload = r#"{"v":1,"records":{"hosts":"[]","names":"{not json","pins":7},"local":{"inyon.a":"1","other":"x","inyon.b":5}}"#;
    let (saved, mut save) = saver();
    let got = import(payload, &store, &mut save).unwrap();
    assert_eq!(got.records, 1);
    assert_eq!(got.skipped, vec!["names: not JSON", "pins: not JSON"]);
    // only inyon.* settings, and only text
    assert_eq!(saved.borrow().keys().collect::<Vec<_>>(), vec!["inyon.a"]);
}

#[test]
fn a_file_this_build_cant_read_imports_nothing() {
    let store = MemoryStore::new();
    assert_eq!(import(r#"{"v":2,"records":{"hosts":"[]"}}"#, &store, &mut |_, _| {}), Err(Refused::Version(2)));
    assert!(matches!(import("INYM garbage", &store, &mut |_, _| {}), Err(Refused::NotJson(_))));
    assert!(store.keys("").is_empty());
}

#[test]
fn the_first_start_imports_then_discards_and_the_core_starts_paired() {
    let dir = temp_dir("first");
    let discarded = Cell::new(0);
    let (saved, mut save) = saver();
    let got = first_start(&dir, || Some(EXPORT.to_owned()), || discarded.set(discarded.get() + 1), &mut save).unwrap().unwrap();
    assert_eq!(got.records, 3);
    assert_eq!(discarded.get(), 1);
    assert_eq!(saved.borrow()["inyon.keepAlive"], "0");

    // the core starts on what was imported: its machines and its settings
    let phone = inyon_phone::native::start(inyon_phone::native::Options {
        dir: dir.join("core"),
        name: "Test phone".into(),
        ssh: Box::new(|| Rc::new(crate::ssh_bridge::BridgeSsh(|_, _, _| false))),
        log: None,
        offline: true,
    });
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let ids = phone.with_phone(|_| async { inyon_core::hosts::registry::registry().configs().into_iter().map(|c| c.id).collect::<Vec<_>>() }).await;
        assert_eq!(ids, Some(vec!["box".to_owned()]));
        let s = phone.request("GET", "/api/settings", None).await.unwrap();
        let s: serde_json::Value = serde_json::from_slice(&s.body).unwrap();
        assert_eq!(s["autoResume"], json!(true));
    });

    // the next start finds the import done: nothing read, nothing written
    let read = Cell::new(false);
    assert!(first_start(
        &dir,
        || {
            read.set(true);
            Some(EXPORT.to_owned())
        },
        || discarded.set(discarded.get() + 1),
        &mut |_, _| panic!("saved again")
    )
    .is_none());
    assert!(!read.get());
    // a file left behind by a stop between the two is discarded then
    assert_eq!(discarded.get(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_file_means_a_fresh_start_and_an_unreadable_one_is_kept() {
    let dir = temp_dir("none");
    let discarded = Cell::new(false);
    assert!(first_start(&dir, || None, || discarded.set(true), &mut |_, _| {}).is_none());
    assert_eq!(first_start(&dir, || Some(r#"{"v":9}"#.into()), || discarded.set(true), &mut |_, _| {}), Some(Err(Refused::Version(9))));
    // kept for a later build, and not marked done
    assert!(!discarded.get());
    assert!(!dir.join(DONE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}
