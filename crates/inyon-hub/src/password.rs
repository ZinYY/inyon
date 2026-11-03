//! Port of password.ts: the hub's sign-in password and the secret that signs its sessions, kept in
//! the `hub` record (~/.inyon/hub.json). The hash is Node's `crypto.scrypt(password, salt, 64)`
//! with its default cost (N 16384, r 8, p 1), so a password set by the Bun hub still signs in.

use std::cell::RefCell;

use inyon_core::platform::store::kv;
use serde_json::{json, Value};

use crate::http::{hex, random_hex, same};

#[derive(Debug, Clone, Default)]
struct HubRecord {
    secret: String,
    password_hash: Option<String>,
    salt: Option<String>,
}

thread_local! {
    static HUB: RefCell<HubRecord> = RefCell::new(HubRecord::default());
}

fn read() -> HubRecord {
    let v = kv::get_value("hub").unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    HubRecord { secret: s("secret").unwrap_or_default(), password_hash: s("passwordHash"), salt: s("salt") }
}

/// Reads the record, making the secret the first time (the module's top level in the TS).
pub fn init() {
    let mut hub = read();
    if hub.secret.is_empty() {
        hub.secret = random_hex(32);
        let mut v = kv::get_value("hub").filter(Value::is_object).unwrap_or_else(|| json!({}));
        v["secret"] = json!(hub.secret);
        if let Err(e) = kv::set_value("hub", &v) {
            inyon_core::platform::console::error(&format!("couldn't save the hub's secret: {e}"));
        }
    }
    HUB.with(|h| *h.borrow_mut() = hub);
}

pub fn session_secret() -> String {
    HUB.with(|h| h.borrow().secret.clone())
}

/// `crypto.scryptSync(password, salt, 64).toString('hex')`
pub fn hash_password(password: &str, salt: &str) -> String {
    let params = scrypt::Params::new(14, 8, 1, 64).expect("scrypt parameters");
    let mut out = [0u8; 64];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).expect("a 64-byte scrypt output");
    hex(&out)
}

/// A new password; the secret changes with it, which signs everyone out.
pub fn set_password(password: &str) -> Result<(), String> {
    let salt = random_hex(16);
    let hub = HubRecord { secret: random_hex(32), password_hash: Some(hash_password(password, &salt)), salt: Some(salt) };
    let v = json!({ "secret": hub.secret, "salt": hub.salt, "passwordHash": hub.password_hash });
    HUB.with(|h| *h.borrow_mut() = hub);
    kv::set_value("hub", &v)
}

pub fn has_password() -> bool {
    HUB.with(|h| h.borrow().password_hash.is_some())
}

/// Whether `password` is the hub's. scrypt runs on a thread of its own, so a burst of guesses
/// doesn't hold up the hub meanwhile.
pub async fn check_password(password: &str) -> bool {
    // one record throughout: set_password may swap it while scrypt runs
    let (salt, hash) = HUB.with(|h| {
        let h = h.borrow();
        (h.salt.clone(), h.password_hash.clone())
    });
    let (Some(salt), Some(hash)) = (salt, hash) else { return false };
    let pw = password.to_owned();
    let got = tokio::task::spawn_blocking(move || hash_password(&pw, &salt)).await.unwrap_or_default();
    // Buffer.from(hash, 'hex') of a well-formed record is the same 128 hex digits
    same(&got, &hash.to_lowercase())
}

/// A password you can type on a phone, and long enough not to guess (`passwordProblem`). Lengths
/// count UTF-16 code units, as JavaScript's do.
pub fn password_problem(pw: &str) -> Option<&'static str> {
    let n = pw.encode_utf16().count();
    if n < 8 {
        Some("Use at least 8 characters.")
    } else if n > 1024 {
        Some("That password is too long.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_nodes_scrypt_with_its_default_cost() {
        // crypto.scryptSync('correct horse', 'c0ffee', 64).toString('hex'), from Bun
        assert_eq!(
            hash_password("correct horse", "c0ffee"),
            "cc29de1984fe31ef6759410e173a4e3a98bdcef45e43332bf587f312b34a3d8775c67bc62aa4fc7bf72c4196bc602b48d412fbe96b87d78953609cb2d35a9b65"
        );
    }

    #[test]
    fn short_and_overlong_passwords_are_refused() {
        assert_eq!(password_problem("1234567"), Some("Use at least 8 characters."));
        assert_eq!(password_problem("12345678"), None);
        assert_eq!(password_problem(&"x".repeat(1025)), Some("That password is too long."));
    }
}
