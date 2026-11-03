//! The organize routes behind the hub's checks: the app (a signed-in browser or a paired phone)
//! may use them; a joined machine's token, a companion and anyone signed out may not.

use std::cell::RefCell;
use std::rc::Rc;

use inyon_core::features::organize;
use inyon_core::hosts::host::Host;
use inyon_core::hosts::registry::{registry, HostRuntime};
use inyon_core::platform::store::kv;
use inyon_protocol::HostState;
use serde_json::{json, Value};

use crate::server::{dispatch, install_ctx, test_req, CtxInit};

const HOST: &str = "hub.test:4777";
const PHONE: &str = "organize-phone-token";
const LAPTOP: &str = "organize-laptop-token";

fn hub() {
    kv::set_value("hub", &json!({ "secret": "organize-test-secret" })).unwrap();
    kv::set_value("settings", &json!({})).unwrap();
    kv::set_value("phones", &json!([{ "id": "ph1", "name": "Pixel", "tokenHash": crate::http::sha256(PHONE), "pairedAt": 1, "lastSeen": 1 }])).unwrap();
    organize::reset();
    kv::del(organize::KEY);
    crate::password::init();
    crate::auth::init();
    crate::phones::init();
    crate::companions::init();
    let mut routes = crate::server::hub_routes();
    crate::core_routes::add(&mut routes);
    install_ctx(
        CtxInit {
            data_dir: std::env::temp_dir().join(format!("inyon-hub-organize-{}", std::process::id())),
            bind: "127.0.0.1".into(),
            port: 0,
            tls: false,
            allowed_hosts: Some("hub.test".into()),
            allowed_origins: None,
            hostname: "test-box".into(),
            setup_code: None,
            local_token: "organize-local".into(),
            hub_commit: String::new(),
            core: inyon_backends::core_here(),
        },
        routes,
    );
    let state: HostState = serde_json::from_value(json!({
        "config": { "id": "laptop", "name": "laptop", "kind": "node", "tokenHash": crate::http::sha256(LAPTOP), "pairedAt": 1, "tokenIssuedAt": 1 },
        "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "work", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [
            { "id": "%3", "index": 0, "active": true, "command": "claude", "cwd": "/w", "title": "claude", "width": 100, "height": 30, "dead": false }
        ] }] }],
    }))
    .unwrap();
    let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state)));
    rt.borrow_mut().hold_polls();
    registry().insert_runtime("laptop", rt);
}

async fn ask(method: &str, path: &str, token: Option<&str>, body: Value, peer: &str) -> (u16, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers = vec![("host", HOST), ("content-type", "application/json")];
    if let Some(a) = &auth {
        headers.push(("authorization", a));
    }
    let mut req = test_req(method, path, &headers, &body.to_string());
    req.peer = peer.parse().unwrap();
    let res = dispatch(req).await;
    let status = res.status().as_u16();
    let bytes = res.into_body().bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[test]
fn only_the_app_reads_and_changes_how_sessions_are_organised() {
    crate::test_util::block(async {
        hub();
        let away = "100.64.0.9:5000";
        let (status, v) = ask("PATCH", "/api/hosts/laptop/panes/3/organize", Some(PHONE), json!({ "pinned": true }), away).await;
        assert_eq!(status, 200, "{v}");
        let (status, v) = ask("GET", "/api/organize", Some(PHONE), json!(null), away).await;
        assert_eq!((status, v["sessions"][0]["pinned"].clone()), (200, json!(0)));

        let (status, _) = ask("GET", "/api/organize", None, json!(null), away).await;
        assert_eq!(status, 401);
        // the inyon command on a joined machine reads sessions, not the app's organising
        for (method, path) in [("GET", "/api/organize"), ("PATCH", "/api/hosts/laptop/panes/3/organize"), ("POST", "/api/organize/projects")] {
            let (status, v) = ask(method, path, Some(LAPTOP), json!({ "name": "x" }), away).await;
            assert_eq!((status, v["reason"].as_str()), (403, Some("scope")), "{method} {path}: {v}");
        }
        assert_eq!(organize::get().projects.len(), 0);
    });
}
