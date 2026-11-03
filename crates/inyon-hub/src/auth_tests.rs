//! auth.test.ts

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::server::test_req;

fn block<F: std::future::Future>(f: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
        f.await
    })
}

fn setup() {
    crate::password::init();
    crate::password::set_password("correct horse battery").unwrap();
}

async fn errors(ip: &str, n: usize) -> Vec<Option<&'static str>> {
    futures_util::future::join_all((0..n).map(|_| try_login(ip, "wrong guess"))).await.into_iter().map(|r| r.err()).collect()
}

#[test]
fn guesses_sent_all_at_once_are_each_counted_and_spread_ones_hit_a_shared_cap() {
    crate::test_util::core();
    setup();
    // the limits count per minute and every guess runs scrypt: on a busy machine the real clock
    // can pass the minute mid-test and reset the counts, so the minute stays put until it moves on purpose
    inyon_core::clock::set_now(Some(now_ms()));
    block(async {
        let out = errors("203.0.113.1", 20).await;
        assert_eq!(out.iter().filter(|e| **e == Some("Wrong password")).count(), 8);
        assert_eq!(out.iter().filter(|e| e.is_some_and(|e| e.starts_with("Too many"))).count(), 12);
        // the right password doesn't get through either until the minute is up
        assert!(try_login("203.0.113.1", "correct horse battery").await.is_err());

        // 8 failures so far; 22 more are allowed across all addresses this minute, then one every few seconds
        let all = futures_util::future::join_all((0..10).map(|i| async move { errors(&format!("198.51.100.{i}"), 4).await })).await;
        let out: Vec<_> = all.into_iter().flatten().collect();
        assert_eq!(out.iter().filter(|e| **e == Some("Wrong password")).count(), 23);
        assert_eq!(try_login("192.0.2.7", "correct horse battery").await, Err(TOO_MANY));
        // a few seconds on, a new address with the right password gets in; a wrong one is refused
        let t = now_ms();
        inyon_core::clock::set_now(Some(t + crate::limits::SLOW_LANE_MS));
        assert!(try_login("192.0.2.7", "correct horse battery").await.is_ok());
        inyon_core::clock::set_now(Some(t + 2 * crate::limits::SLOW_LANE_MS));
        assert_eq!(try_login("192.0.2.8", "not it").await, Err("Wrong password"));
        assert_eq!(try_login("192.0.2.9", "correct horse battery").await, Err(TOO_MANY));
        inyon_core::clock::set_now(None);
    });
}

#[test]
fn past_the_shared_cap_an_address_that_signed_in_before_and_this_machine_carry_on() {
    crate::test_util::core();
    setup();
    inyon_core::clock::set_now(Some(now_ms()));
    block(async {
        assert!(try_login("192.0.2.50", "correct horse battery").await.is_ok());
        let all = futures_util::future::join_all((0..10).map(|i| async move { errors(&format!("198.51.100.{i}"), 4).await })).await;
        assert_eq!(all.into_iter().flatten().filter(|e| *e == Some("Wrong password")).count(), 31);
        assert_eq!(try_login("192.0.2.11", "correct horse battery").await, Err(TOO_MANY));
        assert!(try_login("192.0.2.50", "correct horse battery").await.is_ok());
        assert!(try_login("127.0.0.1", "correct horse battery").await.is_ok());
        // its own limit still holds
        let out = errors("192.0.2.50", 10).await;
        assert_eq!(out.iter().filter(|e| **e == Some("Wrong password")).count(), 8);
    });
    inyon_core::clock::set_now(None);
}

#[test]
fn the_right_password_signs_in() {
    crate::test_util::core();
    setup();
    assert!(block(try_login("192.0.2.9", "correct horse battery")).is_ok());
}

#[test]
fn only_the_hop_the_local_proxy_added_counts_as_the_client() {
    crate::test_util::core();
    assert_eq!(client_address("127.0.0.1", Some("1.2.3.4, 100.64.0.9")), "100.64.0.9");
    assert_eq!(client_address("::1", Some(" 100.64.0.9 ")), "100.64.0.9");
    assert_eq!(client_address("127.0.0.1", None), "127.0.0.1");
    // a client that isn't the proxy can't pick its own address
    assert_eq!(client_address("100.64.0.5", Some("127.0.0.1")), "100.64.0.5");
}

fn cookie_pair() -> String {
    issue_cookie(false).split(';').next().unwrap().to_owned()
}

#[test]
fn signing_out_closes_the_connections_that_session_already_had_open_and_only_those() {
    crate::test_util::core();
    setup();
    let (ca, cb) = (cookie_pair(), cookie_pair());
    let a = test_req("GET", "/ws", &[("cookie", &ca)], "");
    let b = test_req("GET", "/ws", &[("cookie", &cb)], "");
    let closed = Rc::new(RefCell::new(Vec::<String>::new()));
    let socket = |name: &'static str| -> Closer {
        let c = closed.clone();
        Rc::new(move |code, _| c.borrow_mut().push(format!("{name} {code}")))
    };
    let _ = hold_socket(&a, socket("phone"));
    let _ = hold_socket(&a, socket("laptop"));
    let release = hold_socket(&b, socket("other"));
    let _ = hold_socket(&b, socket("another"));
    revoke_session(&a);
    assert_eq!(*closed.borrow(), ["phone 4401", "laptop 4401"]);
    assert!(!is_authed(&a) && is_authed(&b));
    // a connection that already closed isn't closed again; a new password ends all the rest
    release();
    close_sockets(None);
    assert_eq!(*closed.borrow(), ["phone 4401", "laptop 4401", "another 4401"]);
}

#[test]
fn a_cookie_from_before_the_rename_still_signs_you_in_and_comes_back_under_the_new_name() {
    crate::test_util::core();
    setup();
    let value = cookie_pair().split_once('=').unwrap().1.to_owned();
    let old = test_req("GET", "/api/session", &[("cookie", &format!("tether_session={value}"))], "");
    assert!(is_authed(&old));
    assert!(refresh_cookie(&old, false).unwrap().starts_with("inyon_session="));
    // the new one, fresh, isn't sent again
    let new = test_req("GET", "/api/session", &[("cookie", &format!("inyon_session={value}"))], "");
    assert!(refresh_cookie(&new, false).is_none());
}

#[test]
fn a_tampered_or_signed_out_cookie_is_refused_and_a_bearer_is_judged_alone() {
    crate::test_util::core();
    setup();
    let c = cookie_pair();
    let ok = test_req("GET", "/", &[("cookie", &c)], "");
    assert!(is_authed(&ok));
    let tampered = format!("{}x", c);
    assert!(!is_authed(&test_req("GET", "/", &[("cookie", &tampered)], "")));
    // a cookie sent along with a bad bearer doesn't sign it in
    assert!(!is_authed(&test_req("GET", "/", &[("cookie", &c), ("authorization", "Bearer nope")], "")));
    assert!(issue_cookie(true).ends_with("; Secure"));
    assert_eq!(clear_cookie(false), "inyon_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
}

#[test]
fn over_https_the_cookie_is_host_prefixed_and_read_before_a_plain_one() {
    crate::test_util::core();
    setup();
    let secure = issue_cookie(true);
    assert!(secure.starts_with("__Host-inyon_session=") && secure.contains("; Path=/;") && !secure.contains("Domain"));
    assert_eq!(clear_cookie(true), "__Host-inyon_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Secure");
    let value = secure.split(';').next().unwrap().split_once('=').unwrap().1.to_owned();
    // a junk inyon_session set by a neighbour doesn't hide the real one
    let both = test_req("GET", "/api/session", &[("cookie", &format!("inyon_session=junk.value; __Host-inyon_session={value}"))], "");
    assert!(is_authed(&both));
    assert!(refresh_cookie(&both, true).is_none());
    // a browser signed in before keeps its session, and gets it back under the new name over HTTPS
    let plain = test_req("GET", "/api/session", &[("cookie", &format!("inyon_session={value}"))], "");
    assert!(is_authed(&plain));
    assert!(refresh_cookie(&plain, true).unwrap().starts_with("__Host-inyon_session="));
    assert!(refresh_cookie(&plain, false).is_none());
}

fn signed_at(at: u64) -> String {
    let payload = B64U.encode(serde_json::to_string(&json!({ "exp": now_ms() + 86_400_000, "n": "older", "iat": at, "at": at })).unwrap());
    format!("{COOKIE}={payload}.{}", sign(&payload))
}

#[test]
fn pairing_a_phone_takes_a_sign_in_from_the_last_ten_minutes() {
    crate::test_util::core();
    setup();
    let ten = crate::phones::CODE_TTL;
    let fresh = test_req("POST", "/api/phones/pair", &[("cookie", &cookie_pair())], "");
    assert!(is_authed(&fresh) && recent_sign_in(&fresh, ten));
    let old = test_req("POST", "/api/phones/pair", &[("cookie", &signed_at(now_ms() - 11 * 60_000))], "");
    assert!(is_authed(&old));
    assert!(!recent_sign_in(&old, ten));
    assert!(recent_sign_in(&test_req("POST", "/", &[("cookie", &signed_at(now_ms() - 9 * 60_000))], ""), ten));
}

#[test]
fn a_cookie_with_no_at_never_counts_as_a_fresh_sign_in_however_recently_it_was_refreshed() {
    crate::test_util::core();
    setup();
    let payload = B64U.encode(serde_json::to_string(&json!({ "exp": now_ms() + 86_400_000, "n": "legacy", "iat": now_ms() })).unwrap());
    let legacy = test_req("POST", "/api/phones/pair", &[("cookie", &format!("{COOKIE}={payload}.{}", sign(&payload)))], "");
    assert!(is_authed(&legacy));
    assert!(!recent_sign_in(&legacy, crate::phones::CODE_TTL));
}
