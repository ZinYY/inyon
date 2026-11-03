//! Port of limits.ts: the shared part of the guess limits (password, current password, setup code,
//! companion codes). Each has a cap per address and one for all addresses together, so guesses
//! spread over many addresses can't go faster. The shared cap used to be a hard stop, which let
//! anyone who reaches the hub lock the owner out with 30 wrong guesses a minute. Now, past it:
//! - an address that signed in with the password in the last 30 days (and this machine) carries on
//!   under its own per-address limit, and
//! - any other address gets one try every few seconds, shared by all of them, so a new address is
//!   slowed down rather than shut out, and spreading guesses still gains next to nothing.

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;
use inyon_core::platform::store::kv;
use serde_json::{Map, Value};

const KNOWN_FOR_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const KNOWN_MAX: usize = 64;
/// past the shared cap, one try from an unknown address per this long
pub const SLOW_LANE_MS: u64 = 4000;

#[derive(Default)]
struct State {
    /// read from the records on first use
    known: Option<IndexMap<String, u64>>,
    slow_lane: HashMap<String, u64>,
    warned: HashMap<String, u64>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_known<T>(f: impl FnOnce(&mut IndexMap<String, u64>) -> T) -> T {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let known = s.known.get_or_insert_with(|| match kv::get_value("signin-addresses") {
            Some(Value::Object(m)) => m.into_iter().filter_map(|(k, v)| v.as_f64().map(|t| (k, t as u64))).collect(),
            _ => IndexMap::new(),
        });
        f(known)
    })
}

/// This machine, or an address the password was typed from in the last 30 days.
pub fn known_address(ip: &str, now: u64) -> bool {
    crate::auth::loopback(ip) || with_known(|k| k.get(ip).is_some_and(|at| *at > now.saturating_sub(KNOWN_FOR_MS)))
}

/// Remembers an address the right password came from.
pub fn remember_address(ip: &str, now: u64) {
    if ip.is_empty() || ip == "?" || crate::auth::loopback(ip) {
        return;
    }
    let all = with_known(|k| {
        k.shift_remove(ip);
        k.insert(ip.to_owned(), now);
        k.retain(|_, at| *at > now.saturating_sub(KNOWN_FOR_MS));
        while k.len() > KNOWN_MAX {
            k.shift_remove_index(0);
        }
        k.iter().map(|(ip, at)| (ip.clone(), Value::from(*at))).collect::<Map<String, Value>>()
    });
    if let Err(e) = kv::set_value("signin-addresses", &Value::Object(all)) {
        inyon_core::platform::console::error(&format!("couldn't save the addresses that signed in: {e}"));
    }
}

/// Whether a try from `ip` may go ahead while the shared count for `what` stands at `n` against
/// `cap`. Below the cap, always; past it, see the top of this file.
pub fn shared_cap_allows(what: &str, ip: &str, n: u32, cap: u32, now: u64) -> bool {
    if n < cap || known_address(ip, now) {
        return true;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        // the owner should see an attack in the log: once a minute at most
        if s.warned.get(what).is_none_or(|at| *at + 60_000 <= now) {
            s.warned.insert(what.to_owned(), now);
            inyon_core::platform::console::error(&format!(
                "{cap} wrong {what} tries in a minute from several addresses: addresses that signed in before carry on, others get one try every {} s until it calms down",
                SLOW_LANE_MS / 1000
            ));
        }
        if s.slow_lane.get(what).is_some_and(|at| now < at.saturating_add(SLOW_LANE_MS)) {
            return false;
        }
        s.slow_lane.insert(what.to_owned(), now);
        true
    })
}
