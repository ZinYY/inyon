//! `crypto.randomUUID()` for the rare record without an id of its own. These ids only have to be
//! unique within a conversation, so a small generator does, seeded once per thread: from the
//! system's randomness on native builds, and on wasm (which has none the core can reach) from
//! what the host passes to [`seed_ids`], or failing that the clock.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static STATE: Cell<u64> = const { Cell::new(0) };
}

/// Told apart per seeding, so two threads seeding in the same millisecond still differ.
static SEEDINGS: AtomicU64 = AtomicU64::new(0);

/// Seeds this thread's generator, for a wasm host to call once with `crypto.getRandomValues`.
pub fn seed_ids(seed: u64) {
    STATE.with(|s| s.set(mix(seed)));
}

/// splitmix64, so nearby seeds give unrelated states (never 0, which xorshift can't leave)
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)).max(1)
}

#[cfg(not(target_arch = "wasm32"))]
fn entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    // std seeds RandomState from the operating system's randomness
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

#[cfg(target_arch = "wasm32")]
fn entropy() -> u64 {
    crate::clock::now_ms()
}

fn next() -> u64 {
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            x = mix(entropy() ^ SEEDINGS.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x2545_f491_4f6c_dd1d));
        }
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    })
}

/// A version 4 UUID: "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".
pub fn random_uuid() -> String {
    let a = next();
    let b = next();
    let hi = (a & 0xffff_ffff_ffff_0fff) | 0x4000;
    let lo = (b & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", hi >> 32, (hi >> 16) & 0xffff, hi & 0xffff, lo >> 48, lo & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_threads_starting_in_the_same_millisecond_make_different_ids() {
        let run = || {
            std::thread::spawn(|| {
                crate::clock::set_now(Some(1_792_256_430_000));
                (0..4).map(|_| random_uuid()).collect::<Vec<_>>()
            })
        };
        let (a, b) = (run(), run());
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert_ne!(a, b);
        assert!(a.iter().all(|id| id.len() == 36 && &id[14..15] == "4"));
    }

    #[test]
    fn a_seed_from_the_host_decides_the_ids() {
        let ids = |seed| {
            std::thread::spawn(move || {
                seed_ids(seed);
                random_uuid()
            })
            .join()
            .unwrap()
        };
        assert_eq!(ids(7), ids(7));
        assert_ne!(ids(7), ids(8));
    }
}
