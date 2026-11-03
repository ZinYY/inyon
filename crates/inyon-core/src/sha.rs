//! `sha256Hex` from platform/store.ts.

use sha2::{Digest, Sha256};

/// The SHA-256 of `text` as UTF-8, in lower-case hex.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
