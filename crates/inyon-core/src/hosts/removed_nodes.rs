//! Port of hosts/removed-nodes.ts.
//!
//! Token hashes of machines removed from this hub. A machine that was off when it was removed
//! learns it's gone the next time it calls, and takes its own service down, while one whose token
//! the hub simply doesn't know (a hub that lost its data) keeps trying.

use crate::platform::store::kv;

const KEY: &str = "removed-nodes";
const KEEP: usize = 500;

fn list() -> Vec<String> {
    kv::get(KEY).unwrap_or_default()
}

pub fn remember_removed(token_hash: Option<&str>) {
    let Some(hash) = token_hash.filter(|h| !h.is_empty()) else { return };
    let mut l: Vec<String> = list().into_iter().filter(|h| h != hash).collect();
    l.push(hash.to_owned());
    let start = l.len().saturating_sub(KEEP);
    let _ = kv::set(KEY, &l[start..]);
}

/// Compared in constant time, as `crypto.timingSafeEqual` does.
pub fn was_removed(token_hash: &str) -> bool {
    !token_hash.is_empty() && list().iter().any(|h| h.len() == token_hash.len() && h.bytes().zip(token_hash.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_removed_token_is_remembered_once_and_the_list_is_capped() {
        crate::test_support::rt::run(|| async {
            remember_removed(Some("aa"));
            remember_removed(Some("bb"));
            remember_removed(Some("aa"));
            remember_removed(None);
            assert_eq!(list(), vec!["bb", "aa"]);
            assert!(was_removed("aa"));
            assert!(!was_removed("a"));
            assert!(!was_removed(""));
            for i in 0..600 {
                remember_removed(Some(&format!("h{i}")));
            }
            let l = list();
            assert_eq!(l.len(), 500);
            assert_eq!(l.last().map(String::as_str), Some("h599"));
        });
    }
}
