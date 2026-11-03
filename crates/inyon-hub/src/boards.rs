//! Pages' progress boards, looked at once a minute so that a task done on one reaches you as a
//! notice (and a Web Push), on every machine's pages. Each page's count of done tasks is kept in
//! the `page-boards` record, so a restart of the hub neither repeats a notice nor misses one.
//! A page seen for the first time is only noted: adding a page says nothing.

use std::collections::HashMap;

use inyon_core::platform::rt::{sleep, spawn};
use inyon_core::platform::store::kv;
use inyon_protocol::ServerMsg;
use serde_json::Value;

pub const KEY: &str = "page-boards";
/// How often the boards are looked at, in ms.
pub const EVERY_MS: u64 = 60_000;

/// Each page's done count as last seen, by `<host>/<slug>`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Boards(pub HashMap<String, u64>);

impl Boards {
    pub fn load() -> Boards {
        Boards(kv::get::<HashMap<String, u64>>(KEY).unwrap_or_default())
    }

    pub fn save(&self) {
        if let Err(e) = kv::set(KEY, &self.0) {
            inyon_core::platform::console::error(&format!("couldn't keep the pages' boards: {e}"));
        }
    }

    /// The notices for the pages in `entries` (as GET /api/pages lists them) whose board has more
    /// tasks done than last time, and whether anything changed. A page no longer listed is
    /// forgotten when its machine is `here` (connected), and kept while it's away, so a task done
    /// while it was out of reach still comes as a notice when it's back. A page without a board
    /// isn't counted.
    pub fn moved(&mut self, entries: &[Value], here: &dyn Fn(&str) -> bool) -> (Vec<ServerMsg>, bool) {
        let mut out = vec![];
        let mut next = HashMap::new();
        for e in entries {
            let str_of = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
            let (host, slug) = (str_of("host"), str_of("slug"));
            let Some(p) = e.get("progress") else { continue };
            let (Some(done), Some(total)) = (p.get("done").and_then(Value::as_u64), p.get("total").and_then(Value::as_u64)) else { continue };
            let id = format!("{host}/{slug}");
            if self.0.get(&id).is_some_and(|before| done > *before) {
                let now = p.get("now").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty());
                let mut body = if done >= total { format!("All {total} done") } else { format!("{done} of {total} done") };
                if let Some(n) = now.filter(|_| done < total) {
                    body.push_str(&format!(" · {n}"));
                }
                out.push(ServerMsg::PageNotify { host: host.to_owned(), slug: slug.to_owned(), title: str_of("name").to_owned(), body });
            }
            next.insert(id, done);
        }
        for (id, done) in &self.0 {
            let host = id.rsplit_once('/').map_or(id.as_str(), |(h, _)| h);
            if !next.contains_key(id) && !here(host) {
                next.insert(id.clone(), *done);
            }
        }
        let changed = next != self.0;
        self.0 = next;
        (out, changed)
    }
}

/// Looks at the boards once a minute while anyone could hear about them: an app connected, or a
/// browser that asked for notices while it's closed.
pub fn start() {
    spawn(async {
        let mut boards = Boards::load();
        loop {
            sleep(EVERY_MS).await;
            if !crate::clients::any() && crate::push::registrations().iter().all(|r| !r.pages) {
                continue;
            }
            let entries = crate::pages::entries(&crate::pages::pages()).await;
            let connected = crate::pages_remote::machines();
            let here = |host: &str| host == inyon_core::features::pages_core::LOCAL_HOST || connected.iter().any(|m| m == host);
            let (notices, changed) = boards.moved(&entries, &here);
            if changed {
                boards.save();
            }
            for n in &notices {
                crate::clients::announce(n);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(slug: &str, done: u64, total: u64, now: &str) -> Value {
        json!({ "host": "laptop", "slug": slug, "name": format!("Plan {slug}"), "progress": { "done": done, "total": total, "now": now } })
    }

    fn everywhere(_: &str) -> bool {
        true
    }

    fn bodies(n: &[ServerMsg]) -> Vec<String> {
        n.iter()
            .map(|m| match m {
                ServerMsg::PageNotify { host, slug, title, body } => format!("{host}/{slug} {title}: {body}"),
                _ => unreachable!(),
            })
            .collect()
    }

    // "R9 done" should reach the phone, not wait for someone to open the plan
    #[test]
    fn a_task_done_on_a_board_is_a_notice_and_a_new_page_or_a_reset_is_not() {
        let mut b = Boards::default();
        let (n, changed) = b.moved(&[page("robot", 2, 12, "Wiring")], &everywhere);
        assert!(n.is_empty() && changed, "a page seen for the first time is only noted");
        assert_eq!(b.moved(&[page("robot", 2, 12, "Wiring")], &everywhere), (vec![], false));
        assert_eq!(bodies(&b.moved(&[page("robot", 3, 12, "Payment sheet")], &everywhere).0), ["laptop/robot Plan robot: 3 of 12 done · Payment sheet"]);
        // the board was rewritten with fewer done: noted, nothing said
        assert!(b.moved(&[page("robot", 1, 14, "Again")], &everywhere).0.is_empty());
        assert_eq!(bodies(&b.moved(&[page("robot", 14, 14, "Done")], &everywhere).0), ["laptop/robot Plan robot: All 14 done"]);
    }

    #[test]
    fn a_page_without_a_board_or_no_longer_listed_is_forgotten() {
        let mut b = Boards::default();
        b.moved(&[page("robot", 2, 3, ""), page("gone", 1, 3, "")], &everywhere);
        let (_, changed) = b.moved(&[page("robot", 2, 3, ""), json!({ "host": "laptop", "slug": "plain", "name": "Plain" })], &everywhere);
        assert!(changed);
        assert_eq!(b.0, HashMap::from([("laptop/robot".to_owned(), 2)]));
        // back again later: noted afresh, no notice for what it did meanwhile
        assert!(b.moved(&[page("robot", 2, 3, ""), page("gone", 3, 3, "")], &everywhere).0.is_empty());
    }

    // the laptop dropped off for a poll: its pages left the list, then came back further on
    #[test]
    fn a_page_on_a_machine_out_of_reach_keeps_its_count() {
        let mut b = Boards::default();
        b.moved(&[page("robot", 2, 12, "")], &everywhere);
        let away = |h: &str| h != "laptop";
        assert_eq!(b.moved(&[], &away), (vec![], false));
        assert_eq!(bodies(&b.moved(&[page("robot", 3, 12, "")], &everywhere).0), ["laptop/robot Plan robot: 3 of 12 done"]);
    }
}
