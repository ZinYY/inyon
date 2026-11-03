use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use inyon_api::http::test_req;
use inyon_core::platform::store::MemoryStore;
use serde_json::Value;

use super::*;
use crate::testing::{offline, on_core, FakeSsh};

/// The phone started on a fresh core with `store`, as the app starts it.
pub fn on_phone<F: std::future::Future<Output = ()>>(store: Rc<MemoryStore>, test: impl FnOnce(Phone) -> F) {
    on_core(|| test(start(Options { store, fetcher: None, ssh: FakeSsh::new(), reach: offline(), name: "Test phone".into(), rotate_push: Rc::new(|| {}) })));
}

pub async fn body(res: Resp) -> (u16, Value) {
    let status = res.status().as_u16();
    let bytes = res.into_body().bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[test]
fn the_phone_is_its_own_hub_with_nothing_to_sign_in_to() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        assert_eq!(body(p.answer(test_req("GET", "/api/session", &[], "")).await).await, (200, json!({ "authed": true, "phone": true })));
        assert_eq!(body(p.answer(test_req("POST", "/api/logout", &[], "")).await).await.0, 200);
        assert_eq!(body(p.answer(test_req("GET", "/api/nothing", &[], "")).await).await, (404, json!({ "error": "Not found" })));
        // the core has the path, for another method
        assert_eq!(body(p.answer(test_req("DELETE", "/api/settings", &[], "")).await).await, (405, json!({ "error": "Method not allowed" })));
    });
}

#[test]
fn the_cores_routes_keep_their_records_in_the_store_the_app_gave() {
    let store = Rc::new(MemoryStore::new());
    on_phone(store.clone(), |p| async move {
        let (status, _) = body(p.answer(test_req("PUT", "/api/settings", &[("content-type", "application/json")], r#"{"autoResume":true}"#)).await).await;
        assert_eq!(status, 200);
        let (_, got) = body(p.answer(test_req("GET", "/api/settings", &[], "")).await).await;
        assert_eq!(got["autoResume"], json!(true));
    });
    assert_eq!(store.get("settings").unwrap()["autoResume"], json!(true));
}

#[test]
fn a_screen_that_connects_hears_the_machines_from_the_store() {
    let store = Rc::new(MemoryStore::new());
    store.set("hosts", &json!([{ "id": "box", "name": "box", "kind": "ssh", "target": "me@box", "hostKey": "SHA256:x" }])).unwrap();
    on_phone(store, |p| async move {
        let heard = Rc::new(RefCell::new(Vec::<Value>::new()));
        let h = heard.clone();
        let c = p.connect(move |text| h.borrow_mut().push(serde_json::from_str(&text).unwrap()));
        clients::client_message(&c, r#"{"t":"sub.overview"}"#);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let overview = heard.borrow().iter().find(|m| m["t"] == "overview").cloned().expect("an overview");
        assert_eq!(overview["hosts"][0]["config"]["name"], "box");
        // hidden and back, as the app goes to the background and returns
        p.background(true);
        p.background(false);
        p.resume();
        clients::close_client(&c);
        // a closed screen hears nothing more
        let before = heard.borrow().len();
        registry().refresh("box", 0);
        tokio::time::sleep(Duration::from_millis(10_000)).await;
        assert_eq!(heard.borrow().len(), before);
    });
}
