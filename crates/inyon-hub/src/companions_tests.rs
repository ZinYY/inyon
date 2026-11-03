//! companions.test.ts: the code, the claim, the contract header, what a companion may send and
//! receive on /ws, and removal.

use std::cell::RefCell;
use std::rc::Rc;

use inyon_protocol::{NotifyKind, ScreenState};
use serde_json::json;

use super::*;
use crate::server::test_req;

const T0: u64 = 1_700_000_000_000;

fn read(r: Resp) -> (u16, Value) {
    let status = r.status().as_u16();
    let bytes = crate::test_util::block(async move { r.into_body().bytes().await.unwrap() });
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn code_now(now: u64) -> String {
    new_companion_code(now)["code"].as_str().unwrap().to_owned()
}

fn as_companion_req(token: &str, contract: Option<&str>) -> Req {
    let auth = format!("Bearer {token}");
    let mut h = vec![("authorization", auth.as_str())];
    if let Some(c) = contract {
        h.push(("x-inyon-contract", c));
    }
    test_req("GET", "/api/overview", &h, "")
}

#[test]
fn a_code_is_six_characters_without_look_alikes_and_a_new_one_ends_the_last() {
    crate::test_util::core();
    let v = new_companion_code(T0);
    let code = v["code"].as_str().unwrap().to_owned();
    assert!(inyon_core::re!(r"^[2-9A-HJKMNP-Z]{6}$").is_match(&code), "{code}");
    assert_eq!(v["expiresAt"].as_u64(), Some(T0 + COMPANION_CODE_TTL));
    let newer = code_now(T0);
    let (status, body) = read(claim_companion("10.1.0.1", &json!({ "code": code, "name": "Old" }), T0));
    if code != newer {
        assert_eq!((status, body["reason"].clone()), (401, json!("bad-code")));
    }
}

#[test]
fn a_claim_works_once_in_any_case_and_its_token_is_kept_as_a_hash() {
    crate::test_util::core();
    let code = code_now(T0);
    let typed = format!("{}-{} ", code[..3].to_lowercase(), &code[3..]);
    let (status, body) = read(claim_companion("10.1.0.2", &json!({ "code": typed, "name": "  Legato on abhash-laptop  ", "app": "Legato for Windows" }), T0 + 1000));
    assert_eq!(status, 200);
    let token = body["token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 43);
    assert_eq!(body["hubId"], json!(crate::phones::hub_id()));
    assert_eq!(body["scopes"], json!(["overview", "chat", "answer", "send"]));
    assert_eq!(body["contractVersion"], json!(1));
    // plain http: no certificate to pin
    assert!(body.get("tlsFingerprint").is_none());
    let c = companion_for(&as_companion_req(&token, None)).unwrap();
    assert_eq!((c.name.as_str(), c.app.as_deref()), ("Legato on abhash-laptop", Some("Legato for Windows")));
    assert_ne!(c.token_hash, token);
    assert!(companion_for(&as_companion_req(&token[1..], None)).is_none());
    // spent
    let (status, body) = read(claim_companion("10.1.0.2", &json!({ "code": code, "name": "Again" }), T0 + 2000));
    assert_eq!((status, body["reason"].as_str()), (401, Some("bad-code")));
    // listed without the hash
    let listed = list_companions();
    let mut entry = listed.as_array().unwrap().iter().find(|x| x["id"] == json!(c.id)).unwrap().clone();
    // a request moves lastSeen on, by the real clock
    assert!(entry["lastSeen"].as_u64().unwrap() >= T0 + 1000);
    entry.as_object_mut().unwrap().remove("lastSeen");
    assert_eq!(
        entry,
        json!({ "id": c.id, "name": "Legato on abhash-laptop", "app": "Legato for Windows", "scopes": ["overview", "chat", "answer", "send"], "contractVersion": 1, "pairedAt": T0 + 1000 })
    );
    assert!(revoke_companion(&c.id));
    assert!(companion_for(&as_companion_req(&token, None)).is_none());
    assert!(!revoke_companion(&c.id));
}

#[test]
fn an_expired_code_is_refused_and_eight_failures_a_minute_stop_an_address() {
    crate::test_util::core();
    let code = code_now(T0);
    let (status, _) = read(claim_companion("10.1.0.3", &json!({ "code": code }), T0 + COMPANION_CODE_TTL));
    assert_eq!(status, 401);
    let t = T0 + 10 * 60_000;
    for _ in 0..7 {
        assert_eq!(read(claim_companion("10.1.0.4", &json!({ "code": "AAAAAA" }), t)).0, 401);
    }
    let good = code_now(t);
    assert_eq!(read(claim_companion("10.1.0.4", &json!({ "code": "AAAAAA" }), t)).0, 401);
    let (status, body) = read(claim_companion("10.1.0.4", &json!({ "code": good }), t));
    assert_eq!((status, body["reason"].as_str()), (429, Some("rate-limit")));
    // another address still gets in
    assert_eq!(read(claim_companion("10.1.0.5", &json!({ "code": good, "name": "x" }), t)).0, 200);
}

#[test]
fn a_code_with_a_letter_outside_ascii_is_a_wrong_code_not_an_error() {
    crate::test_util::core();
    code_now(T0);
    let (status, body) = read(claim_companion("10.1.0.9", &json!({ "code": "ÄBCDEF" }), T0));
    assert_eq!((status, body["reason"].as_str()), (401, Some("bad-code")));
}

#[test]
fn the_contract_header_or_query_must_name_version_one() {
    crate::test_util::core();
    assert!(contract_refusal(&as_companion_req("x", Some("1")), None).is_none());
    assert!(contract_refusal(&as_companion_req("x", Some(" 1 ")), None).is_none());
    assert!(contract_refusal(&as_companion_req("x", None), Some("1".into())).is_none());
    let (status, body) = read(contract_refusal(&as_companion_req("x", None), None).unwrap());
    assert_eq!(status, 426);
    assert_eq!(body, json!({ "error": "Send the X-Inyon-Contract header. This hub supports companion contract 1.", "reason": "contract", "supported": [1] }));
    let (status, body) = read(contract_refusal(&as_companion_req("x", Some("2")), None).unwrap());
    assert_eq!(status, 426);
    assert_eq!(body["error"], json!("This hub doesn't support companion contract 2. It supports 1."));
}

#[test]
fn a_companion_may_send_only_ping_and_the_subscriptions_its_scopes_allow() {
    let all = SCOPES.to_vec();
    for ok in [r#"{"t":"ping"}"#, r#"{"t":"sub.overview"}"#, r#"{"t":"sub.chat","host":"box","pane":"%1"}"#, r#"{"t":"unsub.chat","host":"box","pane":"%1"}"#] {
        assert_eq!(inbound_refusal(ok, &all), None, "{ok}");
    }
    for bad in [
        r#"{"t":"term.open","id":"1","host":"box","pane":"%1","cols":80,"rows":24}"#,
        r#"{"t":"term.in","id":"1","d":"ls\r"}"#,
        r#"{"t":"draft","host":"box","pane":"%1","text":"x"}"#,
        r#"{"t":"chat.more","host":"box","pane":"%1","before":1}"#,
        r#"{"t":"constructor"}"#,
    ] {
        assert!(inbound_refusal(bad, &all).unwrap().starts_with("Not for companion apps: "), "{bad}");
    }
    assert_eq!(inbound_refusal("not json", &all).unwrap(), "Not for companion apps: an unreadable frame");
    assert!(inbound_refusal(r#"{"t":"sub.chat"}"#, &[CompanionScope::Overview]).is_some());
}

#[test]
fn a_companion_gets_no_history_live_text_drafts_or_terminal_data_and_its_prompts_carry_their_key() {
    let all = SCOPES.to_vec();
    let screen: ScreenState = serde_json::from_value(json!({
        "working": false,
        "width": 80,
        "draft": "half a thought",
        "prompt": { "body": ["Bash command", "rm -rf build", "Do you want to proceed?"], "options": [{ "n": 1, "label": "Yes", "selected": true }, { "n": 2, "label": "No", "selected": false }], "multi": false }
    }))
    .unwrap();
    let v = companion_frame(&ServerMsg::ChatScreen { key: "box:%1".into(), screen: Box::new(screen.clone()) }, &all).unwrap();
    assert_eq!(v["screen"]["prompt"]["key"], json!("Bashcommandrm-rfbuildDoyouwanttoproceed?1.Yes2.No"));
    assert_eq!(v["t"], json!("chat.screen"));
    assert!(v["screen"].get("draft").is_none());
    assert!(companion_frame(&ServerMsg::ChatScreen { key: "box:%1".into(), screen: Box::new(screen) }, &[CompanionScope::Overview]).is_none());
    assert!(companion_frame(&ServerMsg::TermData { id: "1".into(), d: "secret".into() }, &all).is_none());
    assert!(companion_frame(&ServerMsg::ChatAppend { key: "box:%1".into(), items: vec![] }, &all).is_none());
    assert!(companion_frame(&ServerMsg::ChatLive { key: "box:%1".into(), items: vec![] }, &all).is_none());
    assert!(companion_frame(&ServerMsg::ChatProgress { key: "box:%1".into(), done: 1, total: 2 }, &all).is_none());
    assert_eq!(companion_frame(&ServerMsg::Pong, &[]), Some(json!({ "t": "pong" })));
    // the companion agent's reply goes to a companion that may watch chats, without subscribing
    let reply = ServerMsg::CompanionReply { id: "a-1".into(), host: "local".into(), pane: "%9".into(), text: "Two agents need you.".into(), error: None, at: 5 };
    assert_eq!(
        companion_frame(&reply, &[CompanionScope::Chat]),
        Some(json!({ "t": "companion.reply", "id": "a-1", "host": "local", "pane": "%9", "text": "Two agents need you.", "at": 5 }))
    );
    assert!(companion_frame(&reply, &[CompanionScope::Overview, CompanionScope::Answer, CompanionScope::Send]).is_none());
    let notice = ServerMsg::Notify { host: "box".into(), pane: "%1".into(), kind: NotifyKind::Waiting, title: "t".into(), body: "b".into(), agent: None };
    assert!(companion_frame(&notice, &all).is_some());
    assert!(companion_frame(&notice, &[CompanionScope::Chat]).is_none());
}

fn fixture(name: &str) -> ServerMsg {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/protocol/ServerMsg/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn a_chat_reaches_a_companion_without_its_drafts_queue_send_texts_links_or_paths() {
    let all = SCOPES.to_vec();
    let msg = fixture("chat-info");
    let v = companion_frame(&msg, &all).unwrap();
    assert_eq!(keys(&v["info"]), ["agent", "hostId", "mode", "model", "paneId", "running", "sends", "status", "title", "turn"]);
    for s in v["info"]["sends"].as_array().unwrap() {
        assert_eq!(keys(s), ["at", "id", "state"]);
    }
    let ServerMsg::ChatInfo { key, info } = msg else { panic!() };
    let reset = ServerMsg::ChatReset { key, info, items: vec![], has_more: false };
    assert_eq!(companion_frame(&reset, &all).unwrap(), v);
    let ServerMsg::ChatScreen { key, mut screen } = fixture("chat-screen-antigravity-queued") else { panic!() };
    screen.draft = Some("half a thought".into());
    screen.remote = Some(Some("https://claude.ai/code/session_x".into()));
    screen.background = Some(Some("1 shell".into()));
    let v = companion_frame(&ServerMsg::ChatScreen { key, screen }, &all).unwrap();
    assert_eq!(keys(&v["screen"]), ["activity", "effort", "inputVisible", "mode", "model", "width", "working"]);
    assert!(!v.to_string().contains("half a thought"));
}

#[test]
fn the_overview_reaches_a_companion_with_what_labels_a_session_and_no_paths_ids_or_numbers() {
    let v = companion_frame(&fixture("overview"), &SCOPES).unwrap();
    let mut got: std::collections::BTreeMap<&str, std::collections::BTreeSet<String>> = Default::default();
    let mut add = |level: &'static str, o: &Value| got.entry(level).or_default().extend(o.as_object().unwrap().keys().cloned());
    for h in v["hosts"].as_array().unwrap() {
        add("host", h);
        add("config", &h["config"]);
        for s in h["sessions"].as_array().unwrap() {
            add("session", s);
            for w in s["windows"].as_array().unwrap() {
                add("window", w);
                for p in w["panes"].as_array().unwrap() {
                    add("pane", p);
                    if p["agent"].is_object() {
                        add("agent", &p["agent"]);
                    }
                }
            }
        }
    }
    let got: Vec<(&str, Vec<String>)> = got.into_iter().map(|(k, s)| (k, s.into_iter().collect())).collect();
    let want = [
        ("agent", &["activity", "kind", "mode", "options", "state", "waitingFor"][..]),
        ("config", &["id", "kind", "name"]),
        ("host", &["config", "sessions", "status"]),
        ("pane", &["active", "agent", "dead", "id", "index", "label", "title"]),
        ("session", &["id", "name", "windows"]),
        ("window", &["active", "id", "index", "name", "panes"]),
    ];
    assert_eq!(got, want.iter().map(|(k, s)| (*k, s.iter().map(|x| x.to_string()).collect())).collect::<Vec<_>>());
    for secret in ["transcriptPath", "sessionId", "\"pid\"", "\"cwd\"", "identityFile", "tokenHash"] {
        assert!(!v.to_string().contains(secret), "{secret}");
    }
}

#[test]
fn a_companion_types_printable_text_only_and_no_line_of_it_opens_a_command_or_a_shell() {
    let status = |t: &str| text_refusal(t).map(|r| r.status().as_u16());
    for bad in ["\u{15}!id", "x\n\u{1b}[201~\u{15}!id\r", "\u{3}", "tidy up\u{7f}", "hi\u{9b}201~"] {
        assert_eq!(status(bad), Some(400), "{bad:?}");
    }
    for cmd in ["ok\n/model x", "fine\r\n  !rm -rf build", "\u{a0}/clear"] {
        assert_eq!(status(cmd), Some(403), "{cmd:?}");
    }
    assert_eq!(status("Run the tests again.\n\tThen tell me what failed, 100% / 2 or so!"), None);
}

#[test]
fn a_send_that_failed_is_named_by_what_the_hub_said() {
    assert_eq!(send_reason("The agent is waiting for an answer to a question first"), "prompt-open");
    assert_eq!(send_reason("No agent is running in this pane"), "no-agent");
    assert_eq!(send_reason("OpenCode input is not visible. Open the terminal to check before sending."), "busy");
    assert_eq!(send_reason("Change Antigravity settings when it finishes working"), "busy");
    assert_eq!(send_reason("Claude didn't record the message, so it may not have reached it"), "not-landed");
}

#[test]
fn removing_a_companion_closes_its_connections_only() {
    crate::test_util::core();
    let code = code_now(T0);
    let (_, a) = read(claim_companion("10.1.0.6", &json!({ "code": code, "name": "A" }), T0));
    let code = code_now(T0);
    let (_, b) = read(claim_companion("10.1.0.6", &json!({ "code": code, "name": "B" }), T0));
    let a = companion_for(&as_companion_req(a["token"].as_str().unwrap(), None)).unwrap();
    let b = companion_for(&as_companion_req(b["token"].as_str().unwrap(), None)).unwrap();
    let closed: Rc<RefCell<Vec<(String, u16, String)>>> = Rc::new(RefCell::new(vec![]));
    let (ca, cb) = (closed.clone(), closed.clone());
    let _ra = hold_companion_socket(&a.id, Rc::new(move |code, why| ca.borrow_mut().push(("a".into(), code, why.into()))));
    let _rb = hold_companion_socket(&b.id, Rc::new(move |code, why| cb.borrow_mut().push(("b".into(), code, why.into()))));
    assert!(revoke_companion(&a.id));
    assert_eq!(*closed.borrow(), vec![("a".to_owned(), 4001, "Companion removed".to_owned())]);
    assert!(revoke_companion(&b.id));
}

#[test]
fn contract_1_1_adds_the_served_certificate_to_the_code_and_the_claim() {
    let fp = "cd".repeat(32);
    assert_eq!(with_tls(json!({ "code": "K7M2QX" }), Some(fp.clone())), json!({ "code": "K7M2QX", "tlsFingerprint": fp }));
    assert_eq!(with_tls(json!({ "code": "K7M2QX" }), None), json!({ "code": "K7M2QX" }));
}

#[test]
fn wrong_codes_from_many_addresses_slow_new_addresses_down_and_an_address_that_signed_in_carries_on() {
    crate::test_util::core();
    let t = T0 + 20 * 60_000;
    for i in 0..30 {
        assert_eq!(read(claim_companion(&format!("10.2.0.{i}"), &json!({ "code": "AAAAAA" }), t)).0, 401);
    }
    let good = code_now(t);
    // the shared cap is reached: one try from a new address every few seconds
    assert_eq!(read(claim_companion("10.2.1.1", &json!({ "code": "AAAAAA" }), t)).0, 401);
    assert_eq!(read(claim_companion("10.2.1.2", &json!({ "code": good }), t)).1["reason"], "rate-limit");
    // the owner's own computer, where the password was typed, isn't held up
    crate::limits::remember_address("10.2.1.3", t);
    assert_eq!(read(claim_companion("10.2.1.3", &json!({ "code": good, "name": "Legato" }), t)).0, 200);
    let next = code_now(t);
    assert_eq!(read(claim_companion("10.2.1.2", &json!({ "code": next, "name": "Legato" }), t + crate::limits::SLOW_LANE_MS)).0, 200);
}
