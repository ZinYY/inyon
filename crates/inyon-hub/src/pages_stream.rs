//! The `/ws2` pages stream's side in the hub: every machine's pages, read again when an app starts
//! following them, after a page is added or removed here, and every [`EVERY_MS`] while anyone
//! follows them, so `inyon pages add` and a moving board show up without the app asking.

use std::cell::Cell;

use inyon_api::streams;
use inyon_core::platform::rt::{sleep, spawn};
use inyon_protocol::PageEntry;
use serde_json::Value;

/// How often the pages are read again while an app follows them, in ms.
pub const EVERY_MS: u64 = 15_000;

/// One read at a time: a read asked for while one runs (a slow machine can take longer than
/// [`EVERY_MS`]) runs once that one is done, so every read that finishes is delivered and the
/// last one delivered is the newest.
#[derive(Debug, Default)]
pub struct Gate {
    running: Cell<bool>,
    again: Cell<bool>,
}

impl Gate {
    /// Whether to start a read now; false when one runs, which then goes again.
    pub fn start(&self) -> bool {
        if self.running.get() {
            self.again.set(true);
            false
        } else {
            self.running.set(true);
            true
        }
    }

    /// After a read: whether to read again, because one was asked for meanwhile.
    pub fn done(&self) -> bool {
        if self.again.replace(false) {
            true
        } else {
            self.running.set(false);
            false
        }
    }
}

thread_local! {
    static GATE: Gate = Gate::default();
}

/// The list as the API gives it, as the stream's rows. A machine's `progress` this build can't read
/// is dropped from its page rather than the page from the list, as `GET /api/pages` still lists it;
/// an entry that can't be read even then is left out.
pub fn rows(entries: Vec<Value>) -> Vec<PageEntry> {
    entries
        .into_iter()
        .filter_map(|mut v| {
            serde_json::from_value(v.clone()).ok().or_else(|| {
                v.as_object_mut()?.remove("progress")?;
                serde_json::from_value(v).ok()
            })
        })
        .collect()
}

/// Reads every machine's pages and hands them to the stream, while anyone follows it.
pub fn read() {
    if !streams::pages_followed() || !GATE.with(Gate::start) {
        return;
    }
    spawn(async move {
        loop {
            let entries = crate::pages::entries(&crate::pages::pages()).await;
            streams::pages_changed(rows(entries));
            if !GATE.with(Gate::done) {
                break;
            }
        }
    });
}

/// Hooks the stream up: read when followed, and on a timer while it is.
pub fn start() {
    streams::on_pages_wanted(read);
    spawn(async {
        loop {
            sleep(EVERY_MS).await;
            read();
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn the_list_becomes_rows_and_an_entry_this_build_cant_read_is_left_out() {
        let ok = json!({ "host": "local", "slug": "plan", "name": "Plan", "project": "/p", "by": "cli", "added": 5, "folder": "/p/plan", "notes": ["a.md"] });
        let rows = super::rows(vec![ok, json!({ "host": "local" })]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].slug.as_str(), rows[0].notes.clone()), ("plan", Some(vec!["a.md".to_string()])));
    }

    #[test]
    fn a_page_whose_progress_cant_be_read_stays_listed_without_it() {
        let odd = json!({ "host": "laptop", "slug": "plan", "name": "Plan", "project": "/p", "by": "cli", "added": 5, "folder": "/p/plan", "progress": { "done": "three" } });
        let rows = super::rows(vec![odd]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].progress.is_none());
    }

    #[test]
    fn a_read_asked_for_while_one_runs_follows_it_instead_of_being_dropped() {
        let g = super::Gate::default();
        assert!(g.start());
        // the timer fires twice while a slow machine holds the first read
        assert!(!g.start());
        assert!(!g.start());
        assert!(g.done(), "one more read for both");
        assert!(!g.done(), "and then it rests");
        assert!(g.start());
    }
}
