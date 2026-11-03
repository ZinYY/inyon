//! The hub's records as files, its environment, and the core's thread.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use inyon_backends::env::{move_old_folder, HubEnv};
use inyon_backends::fs_store::FsStore;
use inyon_backends::runtime::core_thread;
use inyon_core::platform::store::Store;
use serde_json::json;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("inyon-backends-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn records_are_private_json_files_written_whole() {
    let d = Dir::new("store");
    let data = d.0.join("data");
    let s = FsStore::new(&data).unwrap();
    assert_eq!(mode(&data), 0o700);
    assert_eq!(s.get("hosts"), None);
    s.set("hosts", &json!([{ "id": "local", "b": 1, "a": 2 }])).unwrap();
    // as JSON.stringify writes it: keys in their order, no spaces
    assert_eq!(std::fs::read_to_string(data.join("hosts.json")).unwrap(), r#"[{"id":"local","b":1,"a":2}]"#);
    assert_eq!(mode(&data.join("hosts.json")), 0o600);
    s.set("chats/abc", &json!({ "x": 1 })).unwrap();
    s.set("chats/abd", &json!({ "x": 2 })).unwrap();
    s.set("chats/zzz", &json!({ "x": 3 })).unwrap();
    assert_eq!(mode(&data.join("chats")), 0o700);
    let mut keys = s.keys("chats/ab");
    keys.sort();
    assert_eq!(keys, ["chats/abc", "chats/abd"]);
    let mut top = s.keys("ho");
    top.sort();
    assert_eq!(top, ["hosts"]);
    assert_eq!(s.keys("nothing/"), Vec::<String>::new());
    // no temporary file is left behind
    assert!(std::fs::read_dir(&data).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    s.del("chats/abc");
    assert_eq!(s.get("chats/abc"), None);
    // a record that isn't JSON reads as missing
    std::fs::write(data.join("broken.json"), "{").unwrap();
    assert_eq!(s.get("broken"), None);
}

#[test]
fn tether_variables_count_as_their_inyon_names_and_never_override_them() {
    let env = HubEnv::from_vars([
        ("TETHER_DATA_DIR".to_owned(), "/old".to_owned()),
        ("TETHER_DEBUG".to_owned(), "1".to_owned()),
        ("INYON_DEBUG".to_owned(), String::new()),
        ("HOME".to_owned(), "/home/me".to_owned()),
    ]);
    assert_eq!(env.data_dir(), PathBuf::from("/old"));
    // set (even empty) under the new name: the old one doesn't replace it
    assert!(!env.debug());
    assert_eq!(env.added(), [("INYON_DATA_DIR".to_owned(), "/old".to_owned())]);
    let plain = HubEnv::from_vars([("HOME".to_owned(), "/home/me".to_owned())]);
    assert_eq!(plain.data_dir(), PathBuf::from("/home/me/.inyon"));
    assert_eq!((plain.port(), plain.bind().as_str()), (4777, "127.0.0.1"));
}

#[test]
fn the_old_folder_moves_once_and_leaves_a_link() {
    let d = Dir::new("move");
    std::fs::create_dir(d.0.join(".tether")).unwrap();
    std::fs::write(d.0.join(".tether/hosts.json"), "[]").unwrap();
    move_old_folder(&d.0).unwrap();
    assert_eq!(std::fs::read_to_string(d.0.join(".inyon/hosts.json")).unwrap(), "[]");
    assert!(std::fs::symlink_metadata(d.0.join(".tether")).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_link(d.0.join(".tether")).unwrap(), d.0.join(".inyon"));
    // again, or with nothing to move: things are left alone
    move_old_folder(&d.0).unwrap();
    move_old_folder(&d.0.join("nobody")).unwrap();
    assert!(!d.0.join("nobody").exists());
    assert!(d.0.join(".inyon/hosts.json").exists());
    // a new folder already there: nothing moves
    let e = Dir::new("move-both");
    std::fs::create_dir(e.0.join(".tether")).unwrap();
    std::fs::create_dir(e.0.join(".inyon")).unwrap();
    move_old_folder(&e.0).unwrap();
    assert!(std::fs::symlink_metadata(e.0.join(".tether")).unwrap().is_dir());
}

#[test]
fn work_sent_from_other_threads_runs_on_the_core_thread_in_order() {
    thread_local! {
        static SEEN: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(vec![]) };
    }
    let (core, thread) = core_thread(|| SEEN.with(|s| s.borrow_mut().push(0)));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let got = rt.block_on(async {
        for i in 1..=3 {
            core.send(move || async move { SEEN.with(|s| s.borrow_mut().push(i)) }).unwrap();
        }
        // a job that waits on the core's timers, then reads the core thread's own state
        core.call(|| async {
            inyon_core::platform::rt::sleep(20).await;
            SEEN.with(|s| s.borrow().clone())
        })
        .await
        .unwrap()
    });
    assert_eq!(got, [0, 1, 2, 3]);
    // the test thread's own copy was never touched
    assert!(SEEN.with(|s| s.borrow().is_empty()));
    drop(core);
    thread.join().unwrap();
}

#[test]
fn a_job_that_panics_ends_that_job_not_the_core() {
    let (core, thread) = core_thread(|| {});
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (failed, next) = rt.block_on(async {
        // panics before its first await, on whatever stack runs it first
        let failed = core.call(|| async { panic!("a bug in one request") }).await;
        let next = core.call(|| async { 42 }).await;
        (failed, next)
    });
    assert_eq!(failed, Err::<(), _>(inyon_backends::runtime::CoreGone));
    assert_eq!(next, Ok(42));
    drop(core);
    thread.join().unwrap();
}

#[test]
fn an_old_folder_that_cant_move_is_an_error_not_skipped() {
    let d = Dir::new("stuck");
    let old = d.0.join(".tether");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("hosts.json"), "[]").unwrap();
    // a home that can't be written to: the move fails (install_hub passes this error on, so the hub
    // refuses to start rather than run on an empty ~/.inyon next to the old one)
    std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = move_old_folder(&d.0);
    std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(r.is_err());
    assert!(old.join("hosts.json").exists());
    assert!(!d.0.join(".inyon").exists());
}
