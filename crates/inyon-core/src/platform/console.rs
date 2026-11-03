//! `console.log` and `console.error`: where the core's few lines of logging go. Native builds
//! write them to stderr until the platform says otherwise; wasm has nowhere to write by itself.

use std::cell::RefCell;
use std::rc::Rc;

/// Whether a line is `console.error` (true) or `console.log`.
pub type Sink = Rc<dyn Fn(bool, &str)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Sends this thread's log lines to `sink` (`None`: the default).
pub fn set_sink(sink: Option<Sink>) {
    SINK.with(|s| *s.borrow_mut() = sink);
}

fn write(error: bool, line: &str) {
    match SINK.with(|s| s.borrow().clone()) {
        Some(f) => f(error, line),
        #[cfg(not(target_arch = "wasm32"))]
        None => eprintln!("{line}"),
        #[cfg(target_arch = "wasm32")]
        None => {}
    }
}

/// `console.log(line)`
pub fn log(line: &str) {
    write(false, line);
}

/// `console.error(line)`
pub fn error(line: &str) {
    write(true, line);
}

thread_local! {
    static DEBUG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turns on the extra lines the TS prints with INYON_DEBUG set; the platform reads the variable.
pub fn set_debug(on: bool) {
    DEBUG.with(|d| d.set(on));
}

pub fn debug() -> bool {
    DEBUG.with(std::cell::Cell::get)
}
