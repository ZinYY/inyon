//! hub-update.test.ts, the hub's half ("the hub checks for updates and starts one" and "a hub run
//! as the inyon program"). A development build only says how to update its checkout.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;

fn block<F: std::future::Future>(f: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
        f.await
    })
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tmp() -> Tmp {
    let d = std::env::temp_dir().join(format!("inyon-hub-update-{}", crate::http::random_hex(6)));
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d)
}

fn opts(repo: Option<PathBuf>, data: PathBuf, version: &str) -> Options {
    let mut o = Options::new(repo, data.clone(), String::new());
    o.version = version.into();
    o.status_file = data.join("update-status");
    o.get = Rc::new(|_| Box::pin(async { Err("no network in tests".to_owned()) }));
    o
}

#[test]
fn a_development_build_says_how_to_update_its_checkout_and_starts_nothing() {
    let root = tmp();
    let repo = root.0.join("inyon");
    let (asked, urls) = (Rc::new(RefCell::new(vec![])), Rc::new(RefCell::new(vec![])));
    let mut o = opts(Some(repo.clone()), root.0.join("data"), "0.23.0");
    o.run = no_run(asked.clone(), 0, "");
    o.get = releases(json!("0.24.0"), 200, urls.clone());
    let u = HubUpdater::new(o);
    let why = format!("This hub is a development build from {}. To update it, run git pull and cargo build there, then restart the hub.", repo.display());
    block(async {
        let s = u.status(false).await;
        assert_eq!((s["current"].as_str(), s.get("latest"), s["canUpdate"].as_bool(), s["reason"].as_str()), (Some("0.23.0"), None, Some(false), Some(why.as_str())));
        let res = u.post().await;
        assert_eq!(res.status(), 409);
        assert_eq!(u.start().await, Err(Refused(409, why.clone())));
    });
    assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
    assert!(urls.borrow().is_empty(), "a checkout asked the website: {:?}", urls.borrow());
    assert!(!u.job_file.exists());
}

/// latest.json on the website naming `version`; the addresses asked are noted
fn releases(version: Value, status: u16, urls: Rc<RefCell<Vec<String>>>) -> Get {
    Rc::new(move |url: &str| {
        urls.borrow_mut().push(url.to_owned());
        let body = inyon_core::js::stringify(&json!({ "version": version, "files": [] }));
        Box::pin(async move { Ok((status, body)) })
    })
}

fn no_run(asked: Rc<RefCell<Vec<Vec<String>>>>, code: i32, out: &'static str) -> Exec {
    Rc::new(move |cmd: Vec<String>, _| {
        asked.borrow_mut().push(cmd);
        Box::pin(async move { (code, out.to_owned()) })
    })
}

#[test]
fn looks_for_a_new_release_on_the_website_not_in_a_checkout() {
    let root = tmp();
    let data = root.0.join("bin1");
    let (asked, urls) = (Rc::new(RefCell::new(vec![])), Rc::new(RefCell::new(vec![])));
    let mut o = opts(None, data.clone(), "0.7.0");
    o.run = no_run(asked.clone(), 0, "");
    o.get = releases(json!("0.7.2"), 200, urls.clone());
    let s = block(HubUpdater::new(o).status(false));
    assert_eq!((s["current"].as_str(), s["latest"].as_str(), s["canUpdate"].as_bool()), (Some("0.7.0"), Some("0.7.2"), Some(true)));
    assert!(asked.borrow().is_empty());
    assert_eq!(*urls.borrow(), [LATEST_URL]);
    let mut same = opts(None, data, "0.7.2");
    same.get = releases(json!("0.7.2"), 200, urls);
    assert!(block(HubUpdater::new(same).status(false)).get("latest").is_none());
}

#[test]
fn a_website_that_is_down_or_names_no_release_means_no_update_rather_than_an_error() {
    let root = tmp();
    let urls = Rc::new(RefCell::new(vec![]));
    let html: Get = Rc::new(|_| Box::pin(async { Ok((200, "<html>".to_owned())) }));
    for get in [releases(json!("0.9.0"), 503, urls.clone()), releases(json!("0.9.0-rc.1"), 200, urls.clone()), releases(Value::Null, 200, urls.clone()), html] {
        let mut o = opts(None, root.0.join("bin1b"), "0.7.0");
        o.get = get;
        let s = block(HubUpdater::new(o).status(false));
        assert_eq!((s["current"].as_str(), s.get("latest"), s["canUpdate"].as_bool()), (Some("0.7.0"), None, Some(true)));
    }
}

#[test]
fn post_runs_the_programs_own_update_on_its_own_and_its_status_file_says_how_it_went() {
    let root = tmp();
    let data = root.0.join("bin2");
    let status_file = data.join("node").join("update-status");
    std::fs::create_dir_all(status_file.parent().unwrap()).unwrap();
    let asked = Rc::new(RefCell::new(vec![]));
    let mut o = opts(None, data.clone(), "0.7.0");
    o.run = no_run(asked.clone(), 0, "");
    o.status_file = status_file.clone();
    o.program = "/home/me/.inyon/bin/inyon".into();
    o.get = releases(json!("0.7.2"), 200, Rc::default());
    let u = HubUpdater::new(o);
    block(async {
        assert_eq!(u.post().await.status(), 202);
        assert_eq!(*asked.borrow(), [vec!["/home/me/.inyon/bin/inyon".to_owned(), "update".into(), "--background".into()]]);
        std::fs::write(&status_file, "running\n").unwrap();
        assert_eq!(u.status(false).await["running"], json!(true));
        assert_eq!(u.post().await.status(), 409);
        std::fs::write(&status_file, "failed: The download didn't match the release's checksum, so nothing was changed. Try again.\n").unwrap();
        let s = u.status(false).await;
        assert_eq!(s["error"], json!("The download didn't match the release's checksum, so nothing was changed. Try again."));
        assert_eq!(s["log"], json!(data.join("node").join("update.log").to_string_lossy()));
    });
}

#[test]
fn an_update_that_cant_start_says_why() {
    let root = tmp();
    let mut o = opts(None, root.0.join("bin3"), "0.7.0");
    o.run = no_run(Rc::default(), 1, "systemd-run: not found");
    let u = HubUpdater::new(o);
    block(async {
        assert_eq!(u.start().await, Err(Refused(500, "systemd-run: not found".into())));
        assert_eq!(u.status(false).await["error"], json!("systemd-run: not found"));
    });
}

#[test]
fn a_release_build_is_the_program_even_with_its_checkout_still_on_disk() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(checkout_at(here, true).is_some(), "a development build in this checkout runs from source");
    assert_eq!(checkout_at(here, false), None);
    // a folder that isn't a checkout of inyon
    assert_eq!(checkout_at(&here.join("src"), true), None);
}
