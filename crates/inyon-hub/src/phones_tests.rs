//! phones.test.ts, against a fixed clock.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::json;

use super::*;
use crate::server::test_req;

const T0: u64 = 1_700_000_000_000;
const HUB: &str = "http://localhost:4777";

fn as_phone(token: &str) -> Req {
    test_req("GET", "/api/phones", &[("authorization", &format!("Bearer {token}"))], "")
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

fn pair(ip: &str, name: &str, now: u64) -> (Value, Value) {
    let c = new_pair_code(Some(&json!(HUB)), now).unwrap();
    let r = claim_phone(ip, &json!({ "code": format!("{}:{}", s(&c, "id"), s(&c, "code")), "name": name }), now).unwrap();
    (c, r)
}

#[test]
fn a_code_names_the_hub_and_only_an_http_origin_makes_one() {
    crate::test_util::core();
    let c = new_pair_code(Some(&json!("https://hub.example:4777/")), T0).unwrap();
    assert!(inyon_core::re!(r"^[0-9A-HJKMNP-TV-Z]{8}$").is_match(&s(&c, "id")));
    assert!(inyon_core::re!(r"^\d{6}$").is_match(&s(&c, "code")));
    assert_eq!(c["expiresAt"].as_u64(), Some(T0 + CODE_TTL));
    let link = url::Url::parse(&s(&c, "link")).unwrap();
    assert_eq!(link.scheme(), "inyon");
    let q: std::collections::HashMap<String, String> = link.query_pairs().into_owned().collect();
    assert_eq!(q["u"], "https://hub.example:4777");
    assert_eq!(q["c"], format!("{}:{}", s(&c, "id"), s(&c, "code")));
    assert!(!q["n"].is_empty());
    // the phone shows the hub's id before it pairs; plain http has no certificate to show
    assert_eq!(q["h"], hub_id());
    assert!(!q.contains_key("f"));
    let fp = "ab".repeat(32);
    let with: std::collections::HashMap<String, String> =
        url::Url::parse(&pair_link("https://hub.example", "K7Q4MX2T:493812", Some(&fp))).unwrap().query_pairs().into_owned().collect();
    assert_eq!((with["h"].as_str(), with["f"].as_str()), (hub_id().as_str(), fp.as_str()));
    for bad in [json!("ftp://hub.example"), json!("javascript:alert(1)"), json!("https://hub.example/app"), json!("hub.example"), json!("")] {
        assert_eq!(new_pair_code(Some(&bad), T0).unwrap_err().status, Some(400), "{bad}");
    }
    assert_eq!(new_pair_code(None, T0).unwrap_err().status, Some(400));
}

#[test]
fn a_claim_works_once_with_the_id_in_any_case_and_the_token_signs_that_phone_in() {
    crate::test_util::core();
    let c = new_pair_code(Some(&json!(HUB)), T0).unwrap();
    let r = claim_phone("10.0.0.1", &json!({ "code": format!("{}:{}", s(&c, "id").to_lowercase(), s(&c, "code")), "name": "  Pixel  " }), T0 + 1000).unwrap();
    let token = s(&r, "token");
    assert_eq!(token.len(), 43);
    assert_eq!(r["hub"]["id"], json!(hub_id()));
    assert_eq!(r["hub"]["version"], json!(VERSION));
    assert!(inyon_core::re!(r"^[0-9a-f]{16}$").is_match(&hub_id()));
    let phone = phone_for_at(&as_phone(&token), T0 + 1000).unwrap();
    assert_eq!(phone.name, "Pixel");
    assert!(phone_for(&as_phone(&token[1..])).is_none());
    assert!(phone_for(&test_req("GET", "/api/phones", &[("authorization", &format!("Basic {token}"))], "")).is_none());
    // the code is spent
    let again = claim_phone("10.0.0.1", &json!({ "code": format!("{}:{}", s(&c, "id"), s(&c, "code")), "name": "Again" }), T0 + 2000).unwrap_err();
    assert_eq!((again.status, again.message.as_str()), (Some(404), "That code has expired. Show a new one on the hub."));
    // listed for everyone, marked for the phone asking
    let mine = list_phones_at(&as_phone(&token), T0 + 1000);
    let entry = mine.as_array().unwrap().iter().find(|p| p["id"] == json!(phone.id)).unwrap().clone();
    assert_eq!(entry, json!({ "id": phone.id, "name": "Pixel", "pairedAt": T0 + 1000, "lastSeen": T0 + 1000, "thisPhone": true }));
    let anon = list_phones(&test_req("GET", "/api/phones", &[], ""));
    assert!(!anon.as_array().unwrap().iter().any(|p| p.get("thisPhone").is_some()));
}

#[test]
fn last_seen_moves_on_at_most_once_a_minute() {
    crate::test_util::core();
    let (_, r) = pair("10.0.0.2", "Phone", T0);
    let t = s(&r, "token");
    assert_eq!(phone_for_at(&as_phone(&t), T0 + 30_000).unwrap().last_seen, T0);
    assert_eq!(phone_for_at(&as_phone(&t), T0 + 60_000).unwrap().last_seen, T0 + 60_000);
    assert_eq!(phone_for_at(&as_phone(&t), T0 + 90_000).unwrap().last_seen, T0 + 60_000);
}

#[test]
fn five_wrong_digits_cancel_the_code_unknown_expired_and_garbled_codes_read_alike() {
    crate::test_util::core();
    let c = new_pair_code(Some(&json!(HUB)), T0).unwrap();
    let code: u32 = s(&c, "code").parse().unwrap();
    let wrong = format!("{:06}", (code + 1) % 1_000_000);
    for _ in 0..5 {
        let e = claim_phone("10.0.0.3", &json!({ "code": format!("{}:{wrong}", s(&c, "id")), "name": "x" }), T0).unwrap_err();
        assert_eq!(e.status, Some(401));
    }
    assert_eq!(claim_phone("10.0.0.3", &json!({ "code": format!("{}:{}", s(&c, "id"), s(&c, "code")), "name": "x" }), T0).unwrap_err().status, Some(404));
    let d = new_pair_code(Some(&json!(HUB)), T0).unwrap();
    assert_eq!(claim_phone("10.0.0.4", &json!({ "code": format!("{}:{}", s(&d, "id"), s(&d, "code")), "name": "x" }), T0 + CODE_TTL).unwrap_err().status, Some(404));
    assert_eq!(claim_phone("10.0.0.4", &json!({ "code": "not a code" }), T0).unwrap_err().status, Some(404));
    assert_eq!(claim_phone("10.0.0.4", &json!({}), T0).unwrap_err().status, Some(404));
}

#[test]
fn an_address_gets_eight_failed_claims_a_minute() {
    crate::test_util::core();
    let c = new_pair_code(Some(&json!(HUB)), T0).unwrap();
    let good = json!({ "code": format!("{}:{}", s(&c, "id"), s(&c, "code")), "name": "x" });
    let bad = json!({ "code": "ZZZZZZZZ:000000", "name": "x" });
    for i in 0..8 {
        assert_eq!(claim_phone("10.0.0.5", &bad, T0 + i).unwrap_err().status, Some(404));
    }
    assert_eq!(claim_phone("10.0.0.5", &good, T0 + 10).unwrap_err().status, Some(429));
    // another address is not held up, and after the minute the code (still good) goes through
    assert_eq!(claim_phone("10.0.0.6", &bad, T0 + 10).unwrap_err().status, Some(404));
    assert!(claim_phone("10.0.0.5", &good, T0 + 60_001).unwrap()["token"].is_string());
}

#[test]
fn a_ticket_opens_one_connection_within_thirty_seconds() {
    crate::test_util::core();
    let (_, r) = pair("10.0.0.7", "Phone", T0);
    let phone = phone_for(&as_phone(&s(&r, "token"))).unwrap();
    let t = issue_ticket(&phone.id, T0);
    assert_eq!(redeem_ticket(&t, T0 + 1000).map(|p| p.id), Some(phone.id.clone()));
    assert!(redeem_ticket(&t, T0 + 1000).is_none());
    assert!(redeem_ticket(&issue_ticket(&phone.id, T0), T0 + TICKET_TTL).is_none());
    assert!(redeem_ticket("made-up", T0).is_none());
}

#[test]
fn removing_a_phone_ends_its_token_its_tickets_and_its_connections() {
    crate::test_util::core();
    let (_, r) = pair("10.0.0.8", "Doomed", T0);
    let token = s(&r, "token");
    let phone = phone_for(&as_phone(&token)).unwrap();
    let closed = Rc::new(RefCell::new(Vec::<String>::new()));
    let socket = |name: &'static str| -> Closer {
        let c = closed.clone();
        Rc::new(move |code, reason| c.borrow_mut().push(format!("{name} {code} {reason}")))
    };
    let _ = hold_phone_socket(&phone.id, socket("tab"));
    let release = hold_phone_socket(&phone.id, socket("gone already"));
    release();
    let (_, other) = pair("10.0.0.9", "Other", T0);
    let other_token = s(&other, "token");
    let _ = hold_phone_socket(&phone_for(&as_phone(&other_token)).unwrap().id, socket("other phone"));
    let ticket = issue_ticket(&phone.id, T0);

    assert!(revoke_phone(&phone.id));
    assert_eq!(*closed.borrow(), ["tab 4001 Phone removed"]);
    assert!(phone_for(&as_phone(&token)).is_none());
    assert!(redeem_ticket(&ticket, T0).is_none());
    let names: Vec<Value> = list_phones(&as_phone(&other_token)).as_array().unwrap().iter().map(|p| p["name"].clone()).collect();
    assert!(!names.contains(&json!("Doomed")));
    assert!(!revoke_phone(&phone.id));
}
