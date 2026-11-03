//! The time, for the few readers that compare against it (Codex's elapsed-time spinner, the send
//! ledger). Native builds read the system clock; wasm has none of its own, so the host that embeds
//! the core sets it (`set_now`) before every call that may read it, and tests pin it the same way.
//! A wasm build that reads the clock without it panics in debug builds and reads 0 in release.
//! A thread that runs the stateful core installs its runtime's clock instead (`platform::rt`), so
//! a test on paused time reads that.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Source = Rc<dyn Fn() -> u64>;

thread_local! {
    static NOW: Cell<Option<u64>> = const { Cell::new(None) };
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
}

/// `Date.now()`: milliseconds since the epoch.
pub fn now_ms() -> u64 {
    if let Some(t) = NOW.with(Cell::get) {
        return t;
    }
    if let Some(f) = SOURCE.with(|s| s.borrow().clone()) {
        return f();
    }
    system_ms()
}

/// The clock this thread reads when nothing is pinned (`None` goes back to the system clock).
pub fn set_source(f: Option<Source>) {
    SOURCE.with(|s| *s.borrow_mut() = f);
}

/// Pins the clock for this thread (`None` goes back to the system clock).
pub fn set_now(ms: Option<u64>) {
    NOW.with(|n| n.set(ms));
}

#[cfg(not(target_arch = "wasm32"))]
fn system_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
fn system_ms() -> u64 {
    missing_clock()
}

/// What a read of a clock nobody set gives: loud while developing, 0 (1970) in a release build.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn missing_clock() -> u64 {
    debug_assert!(false, "inyon-core read the time on wasm before the host called clock::set_now");
    0
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "clock::set_now"))]
    fn a_wasm_clock_nobody_set_is_loud_in_debug_builds() {
        assert_eq!(super::missing_clock(), 0);
    }
}
