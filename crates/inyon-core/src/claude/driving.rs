//! Port of claude/driving.ts: panes whose agent menu Inyon itself is working through (a model or
//! mode change goes through the agent's own picker). Those menus aren't questions for you: they're
//! kept off the app and out of "needs your input" notifications while it lasts.

use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static DRIVING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

fn key(host_id: &str, pane_id: &str) -> String {
    format!("{host_id}\0{pane_id}")
}

pub fn is_driving(host_id: &str, pane_id: &str) -> bool {
    DRIVING.with(|d| d.borrow().contains(&key(host_id, pane_id)))
}

/// Starts driving the pane; call the returned function when done.
pub fn start_driving(host_id: &str, pane_id: &str) -> impl FnOnce() {
    let k = key(host_id, pane_id);
    DRIVING.with(|d| d.borrow_mut().insert(k.clone()));
    move || {
        DRIVING.with(|d| d.borrow_mut().remove(&k));
    }
}
