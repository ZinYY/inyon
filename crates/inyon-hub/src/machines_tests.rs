//! The inyon command on the API: a joined machine's token or the hub's local token reads the
//! machines, sessions and pages, and holds a message for an agent only as far as Settings allows.

use inyon_core::platform::store::kv;
use serde_json::{json, Value};

use super::*;
use crate::server::{dispatch, install_ctx, test_req, CtxInit};

const HOST: &str = "hub.test:4777";
const LOCAL: &str = "the-local-token";
const LAPTOP: &str = "laptop-link-token";

fn hub() {
    kv::set_value("hub", &json!({ "secret": "machines-test-secret" })).unwrap();
    kv::set_value("settings", &json!({})).unwrap();
    crate::password::init();
    crate::auth::init();
    crate::phones::init();
    crate::companions::init();
    let mut routes = crate::server::hub_routes();
    crate::core_routes::add(&mut routes);
    install_ctx(
        CtxInit {
            data_dir: std::env::temp_dir().join(format!("inyon-hub-machines-{}", std::process::id())),
            bind: "127.0.0.1".into(),
            port: 0,
            tls: false,
            allowed_hosts: Some("hub.test".into()),
            allowed_origins: None,
            hostname: "test-box".into(),
            setup_code: None,
            local_token: LOCAL.into(),
            hub_commit: String::new(),
            core: inyon_backends::core_here(),
        },
        routes,
    );
    let node = |id: &str, token: &str, paired: Option<u64>, issued: Option<u64>| {
        let mut c: HostConfig = serde_json::from_value(json!({ "id": id, "name": id, "kind": "node" })).unwrap();
        c.token_hash = Some(sha256(token));
        c.paired_at = paired;
        c.token_issued_at = issued;
        registry().add(c, false).unwrap();
    };
    node("laptop", LAPTOP, Some(1), Some(1));
    // a join command nobody ran, two days old: expired
    node("spare", "spare-token", None, Some(now_ms() - 48 * 3_600_000));
}

async fn ask(method: &str, path: &str, token: &str, body: &str, peer: Option<&str>) -> (u16, Value) {
    let auth = format!("Bearer {token}");
    let mut req = test_req(method, path, &[("host", HOST), ("authorization", &auth), ("content-type", "application/json")], body);
    if let Some(p) = peer {
        req.peer = p.parse().unwrap();
    }
    let res = dispatch(req).await;
    let status = res.status().as_u16();
    let bytes = res.into_body().bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[test]
fn a_joined_machines_token_or_the_local_one_is_a_caller_and_nothing_else_is() {
    crate::test_util::block(async {
        hub();
        let req = |token: &str, peer: &str| {
            let auth = format!("Bearer {token}");
            let mut r = test_req("GET", "/api/overview", &[("host", HOST), ("authorization", &auth)], "");
            r.peer = peer.parse().unwrap();
            r
        };
        assert!(matches!(caller(&req(LAPTOP, "100.64.0.9:5000")), Some(Caller::Machine(id)) if id == "laptop"));
        assert_eq!(caller(&req(LOCAL, "127.0.0.1:5000")), Some(Caller::Local));
        // the local token works only from this machine
        assert_eq!(caller(&req(LOCAL, "100.64.0.9:5000")), None);
        assert_eq!(caller(&req("spare-token", "100.64.0.9:5000")), None);
        assert_eq!(caller(&req("nobody", "127.0.0.1:5000")), None);
        assert_eq!(caller(&req(&LAPTOP[1..], "127.0.0.1:5000")), None);
    });
}

#[test]
fn a_machine_reads_the_sessions_but_changes_nothing_else() {
    crate::test_util::block(async {
        hub();
        let (status, v) = ask("GET", "/api/overview", LAPTOP, "", Some("100.64.0.9:5000")).await;
        assert_eq!(status, 200, "{v}");
        let laptop = v["hosts"].as_array().unwrap().iter().find(|h| h["config"]["id"] == "laptop").unwrap().clone();
        // the config without its token hash
        assert_eq!(laptop["config"], json!({ "id": "laptop", "name": "laptop", "kind": "node" }));
        for (method, path, body) in [
            ("GET", "/api/hosts", ""),
            ("GET", "/api/settings", ""),
            ("PUT", "/api/settings", r#"{"machinesSend":true}"#),
            ("POST", "/api/hosts/laptop/panes/1/send", r#"{"text":"hi"}"#),
            ("DELETE", "/api/hosts/laptop", ""),
        ] {
            let (status, v) = ask(method, path, LAPTOP, body, Some("100.64.0.9:5000")).await;
            assert_eq!((status, v["reason"].as_str()), (403, Some("scope")), "{method} {path}: {v}");
        }
        assert!(!sends_allowed());
        // an unknown bearer is still told it was forgotten, as a phone is
        let (status, v) = ask("GET", "/api/overview", "nobody", "", Some("100.64.0.9:5000")).await;
        assert_eq!((status, v["reason"].as_str()), (401, Some("revoked")));
    });
}

#[test]
fn a_message_from_a_machine_waits_for_the_setting_and_the_local_command_needs_none() {
    crate::test_util::block(async {
        hub();
        let held = "/api/hosts/laptop/panes/3/held";
        let (status, v) = ask("POST", held, LAPTOP, r#"{"text":"hi"}"#, Some("100.64.0.9:5000")).await;
        assert_eq!((status, v["reason"].as_str()), (403, Some("send-off")), "{v}");
        assert_eq!(v["error"], json!(SEND_OFF));
        // past the gate, the hold itself answers: no such pane on the laptop
        let (status, v) = ask("POST", held, LOCAL, r#"{"text":"hi"}"#, None).await;
        assert_eq!((status, v["error"].as_str()), (404, Some("Pane not found")), "{v}");
        kv::set_value("settings", &json!({ "machinesSend": true })).unwrap();
        let (status, v) = ask("POST", held, LAPTOP, r#"{"text":"hi"}"#, Some("100.64.0.9:5000")).await;
        assert_eq!((status, v["error"].as_str()), (404, Some("Pane not found")), "{v}");
    });
}

#[test]
fn reading_answering_and_the_companion_are_for_the_local_command_only() {
    crate::test_util::block(async {
        hub();
        for (method, path, body) in [
            ("GET", "/api/hosts/laptop/panes/3/history", ""),
            ("POST", "/api/hosts/laptop/panes/3/answer", r#"{"choice":"1"}"#),
            ("GET", "/api/companion", ""),
            ("POST", "/api/companion", r#"{"agent":"claude"}"#),
            ("DELETE", "/api/companion", ""),
        ] {
            let (status, v) = ask(method, path, LAPTOP, body, Some("100.64.0.9:5000")).await;
            assert_eq!((status, v["reason"].as_str()), (403, Some("local-only")), "{method} {path}: {v}");
        }
        // past the gate, the route itself answers: no such pane, the companion is off
        let (status, v) = ask("GET", "/api/hosts/laptop/panes/3/history", LOCAL, "", None).await;
        assert_eq!((status, v["error"].as_str()), (404, Some("Pane not found")), "{v}");
        let (status, v) = ask("GET", "/api/companion", LOCAL, "", None).await;
        assert_eq!((status, &v["agent"]), (200, &Value::Null), "{v}");
        let (status, v) = ask("POST", "/api/companion", LOCAL, r#"{"agent":"gemini"}"#, None).await;
        assert_eq!((status, v["error"].as_str()), (400, Some("The companion runs Claude Code or Codex")), "{v}");
        // stopping it while it's off changes nothing
        let (status, _) = ask("DELETE", "/api/companion", LOCAL, "", None).await;
        assert_eq!(status, 200);
    });
}
