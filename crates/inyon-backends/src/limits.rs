//! The hub's open-file limit. Every connection, terminal and child process takes descriptors, and
//! a soft limit of 1024 (or macOS's 256) runs out on a busy hub, so it raises its own at start, as
//! Bun does. Its children get the limit back as it was: under systemd or in a container the hard
//! limit can be 524288 or about a billion, and a tmux server, shell or agent that closes every
//! descriptor up to its limit, or sizes a table by it, gets slow with one that high.

#![cfg(unix)]

use std::sync::atomic::{AtomicU64, Ordering};

/// the soft limit before the raise, for children; `u64::MAX` while nothing was raised
static ORIGINAL: AtomicU64 = AtomicU64::new(u64::MAX);
/// macOS refuses a soft limit above OPEN_MAX, even when the hard limit is higher
const MACOS_MAX: u64 = 10240;

/// The soft limit to raise to from `cur` with hard limit `max`, or None to leave it: never lower,
/// and on macOS never above OPEN_MAX.
pub fn raised(cur: u64, max: u64, macos: bool) -> Option<u64> {
    let want = if macos { max.min(MACOS_MAX) } else { max };
    (want > cur).then_some(want)
}

/// Raises this process's soft limit as far as it may go, remembering the old one for children.
/// Returns the limit it now has.
pub fn raise_open_files() -> Option<u64> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit and setrlimit only read and write the struct passed to them
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return None;
    }
    // rlim_t is u64 on the systems the hub runs on, but not on every unix
    #[allow(clippy::unnecessary_cast)]
    let (cur, max) = (lim.rlim_cur as u64, lim.rlim_max as u64);
    let Some(want) = raised(cur, max, cfg!(target_os = "macos")) else { return Some(cur) };
    let up = libc::rlimit { rlim_cur: want as libc::rlim_t, rlim_max: lim.rlim_max };
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &up) } != 0 {
        return Some(cur);
    }
    let _ = ORIGINAL.compare_exchange(u64::MAX, cur, Ordering::SeqCst, Ordering::SeqCst);
    Some(want)
}

/// The soft limit children should have, when this process raised its own.
pub fn for_children() -> Option<u64> {
    Some(ORIGINAL.load(Ordering::SeqCst)).filter(|&n| n != u64::MAX)
}

/// Puts the limit back as it was before the raise. For a `pre_exec` hook: it only reads an atomic
/// and calls setrlimit, both safe between fork and exec.
pub fn restore_in_child() {
    if let Some(n) = for_children() {
        let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: as above; both calls are async-signal-safe
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
                lim.rlim_cur = n as libc::rlim_t;
                libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
            }
        }
    }
}

/// `argv` started through `sh` with the limit put back first, for a spawner with no `pre_exec`
/// hook (the pseudo-terminal's). Unchanged when nothing was raised.
pub fn with_limit_restored(argv: &[String]) -> Vec<String> {
    match for_children() {
        Some(n) => ["/bin/sh".to_owned(), "-c".into(), format!("ulimit -S -n {n} 2>/dev/null; exec \"$@\""), "sh".into()].into_iter().chain(argv.iter().cloned()).collect(),
        None => argv.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_only_ever_goes_up_and_on_macos_to_open_max_at_most() {
        assert_eq!(raised(1024, 524288, false), Some(524288));
        assert_eq!(raised(524288, 524288, false), None);
        assert_eq!(raised(256, u64::MAX, true), Some(10240));
        // a shell that already raised it past OPEN_MAX keeps what it has
        assert_eq!(raised(65536, u64::MAX, true), None);
        assert_eq!(raised(10240, u64::MAX, true), None);
        assert_eq!(raised(256, 4096, true), Some(4096));
    }
}
