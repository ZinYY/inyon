//! The half of claude/transcript-tail.test.ts that follows a conversation through the watcher and
//! its saved copy, with real `tail`, `awk` and `grep` processes reading real files. The machine is
//! seen as a remote one (so image data is left out) but its commands run here. Any tmux the watcher
//! reaches for gets a folder of its own with no server in it, so the user's server is never asked.

#![cfg(unix)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::future::LocalBoxFuture;
use inyon_backends::fs_store::FsStore;
use inyon_backends::process::ProcessBackend;
use inyon_backends::runtime::run_core;
use inyon_core::claude::chat_cache::{load_chat, save_chat, READER};
use inyon_core::claude::watcher::{watchers, ChatWatcher, Sub, Subscriber};
use inyon_core::hosts::host::{set_host_backends, ExecOptions, ExecResult, Host, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use inyon_core::hosts::http_error::HubResult;
use inyon_core::hosts::registry::{registry, HostRuntime};
use inyon_core::platform::rt::{spawn, Deferred};
use inyon_core::platform::store::use_store;
use inyon_protocol::{HostConfig, HostKind, ServerMsg};
use serde_json::{json, Value};

/// A folder of the test's own, removed afterwards.
struct Own(PathBuf);

impl Own {
    fn new(name: &str) -> Own {
        let d = std::env::temp_dir().join(format!("inyon-tail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Own(d)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs every command here, notes each stream's command (the TS spies on `Host.prototype.stream`)
/// and, while `cut` is on, drops a scroll-back read a few lines in.
struct Here {
    real: Rc<ProcessBackend>,
    prefix: String,
    streams: RefCell<Vec<String>>,
    cut: Cell<bool>,
}

fn local(c: &HostConfig) -> HostConfig {
    let mut c = c.clone();
    c.kind = HostKind::Local;
    c
}

impl HostBackend for Here {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        self.real.exec(&local(config), &format!("{}{cmd}", self.prefix), opts)
    }
    fn stream(&self, config: &HostConfig, cmd: &str, mut on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        self.streams.borrow_mut().push(cmd.to_owned());
        let full = format!("{}{cmd}", self.prefix);
        if !self.cut.get() || !cmd.contains("head -c") || cmd.contains("grep -E") {
            return self.real.stream(&local(config), &full, on_line, keep_stdin);
        }
        let mut n = 0;
        let h = self.real.stream(
            &local(config),
            &full,
            Box::new(move |l| {
                n += 1;
                if n <= 6 {
                    on_line(l)
                }
            }),
            keep_stdin,
        );
        let exited = Deferred::new();
        let (real, done) = (h.exited.clone(), exited.clone());
        spawn(async move {
            real.wait().await;
            done.resolve(255);
        });
        StreamHandle::new(move || h.kill(), exited)
    }
    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle> {
        self.real.pty(&local(config), cmd, cols, rows, on_data)
    }
}

impl Here {
    fn cmds(&self) -> Vec<String> {
        self.streams.borrow().clone()
    }
    fn last(&self) -> String {
        self.streams.borrow().last().cloned().unwrap_or_default()
    }
    fn clear(&self) {
        self.streams.borrow_mut().clear();
    }
}

/// The machine "tail-test" with one pane, `pane`, and its records in the test's folder.
fn setup(own: &Own, pane: Value) -> Rc<Here> {
    use_store(Rc::new(FsStore::new(own.file("data")).unwrap()));
    let here = Rc::new(Here {
        real: ProcessBackend::new(own.file("cm"), vec![]),
        prefix: format!("unset TMUX TMUX_PANE; export TMUX_TMPDIR='{}'; ", own.0.display()),
        streams: RefCell::default(),
        cut: Cell::new(false),
    });
    let b = here.clone();
    set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
    let mut full = json!({ "index": 0, "active": true, "command": "claude", "cwd": "/w", "width": 100, "height": 30, "dead": false });
    for (k, v) in pane.as_object().unwrap() {
        full[k] = v.clone();
    }
    let config = json!({ "id": "tail-test", "name": "Remote", "kind": "ssh", "target": "nowhere" });
    let state = serde_json::from_value(json!({
        "config": config, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "work", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [full] }] }],
    }))
    .unwrap();
    let mut rt = HostRuntime::new(Host::new(serde_json::from_value(config).unwrap()), state);
    rt.hold_polls();
    registry().insert_runtime("tail-test", Rc::new(RefCell::new(rt)));
    here
}

fn agent_pane(id: &str, kind: &str, file: &Path, session: &str) -> Value {
    json!({ "id": id, "title": kind, "agent": { "kind": kind, "pid": 1, "state": "idle", "transcriptPath": file.display().to_string(), "sessionId": session } })
}

/// Transcript lines, numbered and stamped a second apart.
#[derive(Default)]
struct Lines {
    n: Cell<u64>,
}

impl Lines {
    fn next(&self) -> u64 {
        self.n.set(self.n.get() + 1);
        self.n.get()
    }
    fn at(&self) -> String {
        inyon_core::js::date::to_iso(1_800_000_000_000.0 + self.next() as f64 * 1000.0).unwrap()
    }
    fn user(&self, text: &str, image: bool) -> String {
        let n = self.next();
        let mut content = vec![json!({ "type": "text", "text": text })];
        if image {
            content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": format!("iVBORw0KGgo{}", "A".repeat(50_000)) } }));
        }
        let ts = inyon_core::js::date::to_iso(1_800_000_000_000.0 + n as f64 * 1000.0).unwrap();
        format!("{}\n", json!({ "type": "user", "uuid": format!("u{n}"), "timestamp": ts, "message": { "role": "user", "content": content } }))
    }
    fn goal(&self, condition: &str, met: bool) -> String {
        let n = self.next();
        let ts = inyon_core::js::date::to_iso(1_800_000_000_000.0 + n as f64 * 1000.0).unwrap();
        format!("{}\n", json!({ "type": "attachment", "uuid": format!("g{n}"), "timestamp": ts, "attachment": { "type": "goal_status", "condition": condition, "met": met } }))
    }
}

fn append(file: &Path, text: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(file).unwrap().write_all(text.as_bytes()).unwrap();
}

fn size(file: &Path) -> u64 {
    std::fs::metadata(file).unwrap().len()
}

/// A phone watching the chat; what it got, as the JSON it would read.
#[derive(Default)]
struct Phone {
    got: RefCell<Vec<Value>>,
}

impl Subscriber for Phone {
    fn send(&self, m: &ServerMsg) {
        self.got.borrow_mut().push(serde_json::to_value(m).unwrap());
    }
}

impl Phone {
    fn all(&self) -> Vec<Value> {
        self.got.borrow().clone()
    }
    fn of(&self, t: &str) -> Vec<Value> {
        self.got.borrow().iter().filter(|m| m["t"] == t).cloned().collect()
    }
    fn any(&self, t: &str) -> bool {
        self.got.borrow().iter().any(|m| m["t"] == t)
    }
    fn first(&self, t: &str) -> Value {
        self.of(t).first().cloned().unwrap_or(Value::Null)
    }
    fn last(&self, t: &str) -> Option<Value> {
        self.of(t).last().cloned()
    }
    fn clear(&self) {
        self.got.borrow_mut().clear();
    }
    /// The first line of each user message in every message of this kind.
    fn texts(&self, t: &str) -> Vec<String> {
        self.of(t).iter().flat_map(|m| users(m).into_iter().map(|s| s.split('\n').next().unwrap().to_owned())).collect()
    }
    /// What's known about the conversation as of the last info or reset.
    fn latest(&self) -> Value {
        self.got.borrow().iter().rev().find(|m| m["t"] == "chat.info" || m["t"] == "chat.reset").map(|m| m["info"].clone()).unwrap_or(Value::Null)
    }
    fn goals(&self) -> Vec<Value> {
        self.got.borrow().iter().filter(|m| m["t"] == "chat.info" || m["t"] == "chat.reset").map(|m| m["info"]["goal"].clone()).collect()
    }
}

fn user_items(items: &Value) -> Vec<Value> {
    items.as_array().unwrap().iter().filter(|i| i["kind"] == "user").cloned().collect()
}

fn users(m: &Value) -> Vec<String> {
    user_items(&m["items"]).iter().map(|i| i["text"].as_str().unwrap().to_owned()).collect()
}

fn items(w: &ChatWatcher) -> Value {
    serde_json::to_value(w.items()).unwrap()
}

fn num(text: &str) -> Option<u64> {
    text.strip_prefix("msg ")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn nums(items: &Value) -> Vec<u64> {
    user_items(items).iter().filter_map(|i| num(i["text"].as_str().unwrap())).collect()
}

fn first_seq(w: &ChatWatcher) -> i64 {
    items(w)[0]["seq"].as_i64().unwrap()
}

/// Scrolls up until the start of the conversation.
async fn scroll_to_start(w: &ChatWatcher, phone: &Phone, sub: &Sub) {
    while phone.last("chat.more").is_none_or(|m| m["hasMore"] == true) {
        w.more(sub, first_seq(w)).await;
    }
}

fn phone() -> (Rc<Phone>, Sub) {
    let p = Rc::new(Phone::default());
    let s: Sub = p.clone();
    (p, s)
}

macro_rules! until {
    ($tries:expr, $ok:expr) => {{
        let mut i = 0;
        while i < $tries && !$ok {
            tokio::time::sleep(Duration::from_millis(25)).await;
            i += 1;
        }
        assert!($ok, "never came true: {}", stringify!($ok));
    }};
}

#[test]
fn a_dropped_connection_carries_on_from_where_it_stopped_instead_of_reading_it_all_again() {
    let own = Own::new("dropped");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("chat.jsonl");
        std::fs::write(&file, l.user("first", false) + &l.user("with a picture", true) + &l.user("second", false)).unwrap();
        let here = setup(&own, agent_pane("%1", "claude", &file, "s1"));
        let (got, sub) = phone();
        let first_lines = |w: &ChatWatcher| user_items(&items(w)).iter().map(|i| i["text"].as_str().unwrap().split('\n').next().unwrap().to_owned()).collect::<Vec<_>>();

        let w = watchers().get("tail-test", "%1");
        w.subscribe(sub.clone());
        until!(200, got.texts("chat.reset").len() == 3);
        assert_eq!(got.texts("chat.reset"), ["first", "with a picture", "second"]);
        assert!(here.last().contains("tail -c +1 "));
        assert!(here.last().contains("awk"));

        // the connection drops; more is said meanwhile; it comes back
        w.kill_tail();
        until!(200, w.tail_lost());
        append(&file, &l.user("while away", false));
        w.sync();
        until!(200, got.texts("chat.append").iter().any(|t| t == "while away"));
        assert!(!here.last().contains("tail -c +1 "));
        assert_eq!(got.of("chat.reset").len(), 1);
        assert_eq!(first_lines(&w), ["first", "with a picture", "second", "while away"]);

        // closed and opened again within half an hour: the next watcher carries on too
        watchers().stop(&w.key);
        append(&file, &l.user("later", false));
        let again = watchers().get("tail-test", "%1");
        got.clear();
        again.subscribe(sub.clone());
        assert_eq!(got.texts("chat.reset"), ["first", "with a picture", "second", "while away"]);
        until!(200, got.texts("chat.append").iter().any(|t| t == "later"));
        assert!(!here.last().contains("tail -c +1 "));

        // the count is off (the file was replaced by one as long): read it again rather than mid-line
        again.kill_tail();
        until!(200, again.tail_lost());
        again.shift_reader(-3);
        got.clear();
        again.sync();
        until!(200, got.texts("chat.reset").len() == 5);
        assert_eq!(got.texts("chat.reset"), ["first", "with a picture", "second", "while away", "later"]);

        // a file that got shorter was rewritten: read it again from the start
        again.kill_tail();
        until!(200, again.tail_lost());
        got.clear();
        std::fs::write(&file, l.user("fresh", false)).unwrap();
        again.sync();
        until!(200, got.texts("chat.reset").iter().any(|t| t == "fresh"));
        assert_eq!(first_lines(&again), ["fresh"]);
        watchers().stop(&again.key);
    });
}

#[test]
fn a_long_backlog_is_read_out_of_sight_and_shown_at_once_the_saved_place_survives_a_restart() {
    let own = Own::new("backlog");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("away.jsonl");
        std::fs::write(&file, (0..30).map(|i| l.user(&format!("m{i}"), false)).collect::<String>()).unwrap();
        let here = setup(&own, agent_pane("%2", "claude", &file, "s2"));
        let (got, sub) = phone();

        let w = watchers().get("tail-test", "%2");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));

        // hours of work while away, with a goal set and met on the way
        w.kill_tail();
        until!(400, w.tail_lost());
        let mut backlog = l.goal("ship it", false);
        for i in 0..300 {
            backlog += &l.user(&format!("away {i} {}", "x".repeat(300)), false);
        }
        backlog += &l.goal("ship it", true);
        append(&file, &backlog);
        got.clear();
        w.sync();
        until!(400, got.any("chat.reset"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        // nothing replayed message by message, and no passing state (the active goal) was ever shown
        assert_eq!(got.of("chat.append").len(), 0);
        assert!(!got.of("chat.info").iter().any(|m| m["info"]["goal"].is_object() && m["info"]["goal"]["met"] != true));
        assert!(got.any("chat.progress"));
        let reset = got.first("chat.reset");
        assert_eq!(reset["info"]["goal"]["met"], true);
        assert!(users(&reset).last().unwrap().starts_with("away 299"));

        // a small catch-up still streams in as it's read
        w.kill_tail();
        until!(400, w.tail_lost());
        append(&file, &l.user("just one", false));
        got.clear();
        w.sync();
        until!(400, got.of("chat.append").iter().any(|m| users(m).iter().any(|u| u == "just one")));
        assert_eq!(got.of("chat.reset").len(), 0);

        // the app restarts (or comes back hours later): its saved copy says where reading stopped
        let pairs = |w: &ChatWatcher| items(w).as_array().unwrap().iter().map(|i| (i["seq"].clone(), i["uuid"].clone())).collect::<Vec<_>>();
        let all = pairs(&w);
        watchers().stop(&w.key);
        watchers().clear_parked();
        append(&file, &l.user("after the restart", false));
        got.clear();
        let fresh = watchers().get("tail-test", "%2");
        fresh.subscribe(sub.clone());
        let first = got.first("chat.reset");
        assert_eq!(users(&first).last().unwrap(), "just one");
        assert_eq!(first["hasMore"], true);
        until!(400, got.of("chat.append").iter().any(|m| users(m).iter().any(|u| u == "after the restart")));
        assert!(!here.last().contains("tail -c +1 "));
        assert!(fresh.items().len() < all.len());

        // earlier messages than the saved copy holds come from the file, numbered as before
        while got.last("chat.more").is_none_or(|m| m["hasMore"] == true) {
            let n0 = got.all().len();
            fresh.more(&sub, first_seq(&fresh)).await;
            assert_eq!(got.all().len(), n0 + 1);
        }
        let now = pairs(&fresh);
        assert_eq!(now[..all.len()], all[..]);
        assert_eq!(items(&fresh)[0]["seq"], 0);
        watchers().stop(&fresh.key);
    });
}

#[test]
fn a_long_conversation_opens_on_its_last_stretch_and_reads_earlier_messages_as_you_scroll_up() {
    let own = Own::new("long");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("long.jsonl");
        let goal = format!(
            "{}\n",
            json!({ "type": "attachment", "uuid": format!("g{}", l.n.get()), "timestamp": l.at(), "attachment": { "type": "goal_status", "condition": "early goal", "met": false } })
        );
        let turn = || {
            let n = l.next();
            format!("{}\n", json!({ "type": "system", "subtype": "turn_duration", "uuid": format!("t{n}"), "timestamp": l.at(), "durationMs": 1000 }))
        };
        // about 2.4 MB: well past the last stretch read at first, with a screenshot in the middle
        let mut text = goal + &turn();
        for i in 0..2000 {
            text += &l.user(&format!("msg {i} {}", "x".repeat(1100)), i == 1000);
            if i % 100 == 0 {
                text += &turn();
            }
        }
        std::fs::write(&file, text).unwrap();
        assert!(size(&file) > 2_000_000);
        let here = setup(&own, agent_pane("%3", "claude", &file, "s3"));
        let (got, sub) = phone();

        let t0 = Instant::now();
        let w = watchers().get("tail-test", "%3");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        let opened = t0.elapsed();
        let reset = got.first("chat.reset");
        // nothing read the whole file: a summary of the early part, then the last stretch
        assert!(!here.cmds().iter().any(|c| c.contains("tail -c +1 ")));
        assert!(here.cmds().iter().any(|c| c.contains("grep -E")));
        assert_eq!(nums(&reset["items"]).last(), Some(&1999));
        assert_eq!(reset["hasMore"], true);
        // what's known about the whole conversation follows a moment later, as if it were all read
        until!(400, got.latest()["goal"]["condition"] == "early goal");
        assert_eq!(got.latest()["stats"]["turns"], 21);
        assert!(w.items().len() < 2000);

        // scrolling up: every earlier message once, in order, until the start of the file
        scroll_to_start(&w, &got, &sub).await;
        let all = items(&w);
        assert_eq!(nums(&all), (0..2000).collect::<Vec<_>>());
        let seqs: Vec<i64> = all.as_array().unwrap().iter().map(|i| i["seq"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|p| p[1] > p[0]));
        let shot = user_items(&all).into_iter().find(|i| num(i["text"].as_str().unwrap()) == Some(1000)).unwrap();
        assert_eq!(shot["images"], 1);
        println!("opened a {:.1} MB conversation in {} ms", size(&file) as f64 / 1e6, opened.as_millis());
        watchers().stop(&w.key);

        // a copy saved before this doesn't say where its first message is: it's read afresh, not scrolled from
        watchers().clear_parked();
        let mut saved = load_chat(&w.key).unwrap();
        let mut kept = saved.clone();
        assert!(saved.at.as_ref().unwrap().older.unwrap_or(0) > 0);
        saved.at.as_mut().unwrap().older = None;
        save_chat(&w.key, saved);
        here.clear();
        got.clear();
        let again = watchers().get("tail-test", "%3");
        again.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        assert!(here.cmds().iter().any(|c| c.contains("grep -E")));
        assert_eq!(got.first("chat.reset")["hasMore"], true);
        watchers().stop(&again.key);

        // a copy whose summary an older reader built is read afresh too, so a fixed mistake doesn't stay
        watchers().clear_parked();
        assert_eq!(kept.at.as_ref().unwrap().reader, Some(READER));
        kept.at.as_mut().unwrap().reader = Some(READER - 1);
        save_chat(&w.key, kept);
        here.clear();
        got.clear();
        let third = watchers().get("tail-test", "%3");
        third.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        assert!(here.cmds().iter().any(|c| c.contains("grep -E")));
        watchers().stop(&third.key);
    });
}

#[test]
fn a_long_codex_thread_opens_on_its_last_stretch_too_with_its_summary_and_pages_back_to_the_start() {
    let own = Own::new("codex");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("long-codex.jsonl");
        let rec = |kind: &str, payload: Value| format!("{}\n", json!({ "timestamp": l.at(), "type": kind, "payload": payload }));
        let say = |role: &str, text: &str| {
            let id = format!("msg_{}", l.n.get());
            rec(
                "response_item",
                json!({ "type": "message", "id": id, "role": role, "content": [{ "type": if role == "user" { "input_text" } else { "output_text" }, "text": text }] }),
            )
        };
        let mut text = rec("session_meta", json!({ "id": "thread", "cwd": "/home/me/play", "cli_version": "0.163.0" }));
        text += &rec("event_msg", json!({ "type": "thread_goal_updated", "goal": { "objective": "early goal", "status": "active" } }));
        text += &rec(
            "event_msg",
            json!({ "type": "item_completed", "item": { "type": "FileChange", "id": "fc-early", "status": "completed", "changes": { "/home/me/play/a.txt": { "type": "add", "content": "one\ntwo\n" } } } }),
        );
        for i in 0..600 {
            text += &say("user", &format!("msg {i} {}", "x".repeat(1100)));
            text += &say("assistant", &format!("reply {i}"));
            text += &rec("event_msg", json!({ "type": "task_complete", "turn_id": format!("t{i}"), "duration_ms": 1000 }));
        }
        std::fs::write(&file, text).unwrap();
        assert!(size(&file) > 600_000);
        let here = setup(&own, agent_pane("%7", "codex", &file, "thread"));
        let (got, sub) = phone();

        let w = watchers().get("tail-test", "%7");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        let reset = got.first("chat.reset");
        let cmds = here.cmds();
        // the last stretch and a grep for the summary lines, never the whole file; no live helper for a
        // file outside Codex's own folder
        assert!(!cmds.iter().any(|c| c.contains("tail -c +1 ")));
        assert!(cmds.iter().any(|c| c.contains("grep -E") && c.contains("thread_goal_updated")));
        assert!(!cmds.iter().any(|c| c.contains("node -e")));
        assert_eq!(nums(&reset["items"]).last(), Some(&599));
        assert_eq!(reset["hasMore"], true);
        until!(400, got.latest()["goal"]["condition"] == "early goal");
        assert_eq!(got.latest()["stats"]["turns"], 600);
        assert_eq!(got.latest()["stats"]["linesAdded"], 2);
        assert!(w.items().len() < 1200);

        scroll_to_start(&w, &got, &sub).await;
        let all = items(&w);
        assert_eq!(nums(&all), (0..600).collect::<Vec<_>>());
        let seqs: Vec<i64> = all.as_array().unwrap().iter().map(|i| i["seq"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|p| p[1] > p[0]));
        watchers().stop(&w.key);
    });
}

#[test]
fn a_scroll_back_read_cut_short_keeps_nothing_and_the_next_scroll_up_reads_that_piece_again() {
    let own = Own::new("drop");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("drop.jsonl");
        std::fs::write(&file, (0..600).map(|i| l.user(&format!("msg {i} {}", "y".repeat(1100)), false)).collect::<String>()).unwrap();
        let here = setup(&own, agent_pane("%6", "claude", &file, "s6"));
        let (got, sub) = phone();
        let w = watchers().get("tail-test", "%6");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        let first_held = nums(&items(&w))[0];
        assert!(first_held > 0);

        // the connection drops a few lines into the next piece back
        here.cut.set(true);
        w.more(&sub, first_seq(&w)).await;
        assert_eq!(nums(&items(&w))[0], first_held);
        assert_eq!(got.last("chat.more").unwrap()["hasMore"], true);

        // back online: scrolling up reads every earlier message, none skipped
        here.cut.set(false);
        scroll_to_start(&w, &got, &sub).await;
        assert_eq!(nums(&items(&w)), (0..600).collect::<Vec<_>>());
        watchers().stop(&w.key);
    });
}

#[test]
fn a_reopened_chat_shows_its_saved_copy_at_once_and_reads_only_what_was_said_since() {
    let own = Own::new("reopen");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("reopen.jsonl");
        let goal = format!(
            "{}\n",
            json!({ "type": "attachment", "uuid": format!("g{}", l.n.get()), "timestamp": l.at(), "attachment": { "type": "goal_status", "condition": "kept goal", "met": false } })
        );
        let say = |from: u64, count: u64| (from..from + count).map(|i| l.user(&format!("msg {i} {}", "word ".repeat(220)), false)).collect::<String>();
        std::fs::write(&file, goal + &say(0, 600)).unwrap();
        let here = setup(&own, agent_pane("%4", "claude", &file, "s4"));
        let (got, sub) = phone();
        let last_user = |m: &Value| nums(&m["items"]).last().copied();

        let w = watchers().get("tail-test", "%4");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset"));
        // saved as soon as it's in, summary included, not 10 s later: an app closed by then keeps it
        until!(400, load_chat(&w.key).and_then(|c| c.at).is_some_and(|at| at.offset == size(&file) && at.meta["goal"].is_object()));
        watchers().stop(&w.key);
        watchers().clear_parked();

        // hours later: far more was said than the last stretch holds
        let before = size(&file);
        append(&file, &say(600, 400));
        here.clear();
        got.clear();
        let again = watchers().get("tail-test", "%4");
        again.subscribe(sub.clone());
        // the saved chat shows at once, then the latest
        assert_eq!(last_user(&got.first("chat.reset")), Some(599));
        until!(400, got.of("chat.reset").iter().any(|m| last_user(m) == Some(999)));
        let cmds = here.cmds();
        // only the last stretch and a summary of what's between it and the saved copy, never from the start
        assert!(cmds.iter().any(|c| c.contains("grep -E") && c.contains(&format!("tail -c +{} ", before + 1))));
        assert!(!cmds.iter().any(|c| c.contains("head -c") && !c.contains("tail -c")));
        until!(400, got.latest()["goal"]["condition"] == "kept goal");
        // scrolling up still reaches every message, through the part that was skipped
        scroll_to_start(&again, &got, &sub).await;
        assert_eq!(nums(&items(&again)), (0..1000).collect::<Vec<_>>());
        watchers().stop(&again.key);
    });
}

#[test]
fn a_reopened_chat_never_flashes_a_goal_that_was_met_while_it_was_closed() {
    let own = Own::new("goal");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("goal.jsonl");
        std::fs::write(&file, l.user("start", false) + &l.goal("ship it", false) + &l.user("working on it", false)).unwrap();
        setup(&own, agent_pane("%5", "claude", &file, "s5"));
        let (got, sub) = phone();
        let active = |g: &Value| g["condition"] == "ship it" && g["met"] != true;
        let reopen = || async {
            watchers().stop("tail-test:%5");
            watchers().clear_parked();
            got.clear();
            let w = watchers().get("tail-test", "%5");
            w.subscribe(sub.clone());
            tokio::time::sleep(Duration::from_millis(400)).await;
            w
        };

        let w = watchers().get("tail-test", "%5");
        w.subscribe(sub.clone());
        until!(400, got.goals().iter().any(active) && load_chat(&w.key).and_then(|c| c.at).is_some_and(|at| at.offset == size(&file)));

        // nothing new: the goal is still on, and shows
        reopen().await;
        until!(400, got.goals().iter().any(active));

        // met while the app was closed: the old goal never shows as active
        append(&file, &(l.user("done", false) + &l.goal("ship it", true)));
        reopen().await;
        until!(400, got.any("chat.reset"));
        assert!(!got.goals().iter().any(|g| g.is_object() && g["met"] != true));
        watchers().stop("tail-test:%5");
    });
}

#[test]
fn scrolling_back_past_where_numbering_started_keeps_every_message_in_order() {
    let own = Own::new("negative");
    run_core(|| async {
        let l = Lines::default();
        let file = own.file("neg.jsonl");
        let lines: Vec<String> = (0..30).map(|i| l.user(&format!("msg {i}"), false)).collect();
        std::fs::write(&file, lines.concat()).unwrap();
        setup(&own, agent_pane("%8", "claude", &file, "s8"));
        let (got, sub) = phone();
        let w = watchers().get("tail-test", "%8");
        w.subscribe(sub.clone());
        until!(400, got.any("chat.reset") && load_chat(&w.key).and_then(|c| c.at).is_some_and(|at| at.offset == size(&file)));
        watchers().stop(&w.key);
        watchers().clear_parked();

        // a saved copy whose numbering starts lower than the messages before it (the file was
        // rewritten, or a newer reader makes more items of the same lines): the last three, from 1
        let mut saved = load_chat(&w.key).unwrap();
        let keep: Vec<_> = saved.items.split_off(saved.items.len() - 3);
        saved.items = keep.into_iter().enumerate().map(|(i, it)| inyon_core::claude::transcript::with_seq(it, i as i64 + 1)).collect();
        let at = saved.at.as_mut().unwrap();
        at.older = Some(lines[..27].concat().len() as u64);
        at.seq = 4;
        save_chat(&w.key, saved);

        got.clear();
        let again = watchers().get("tail-test", "%8");
        again.subscribe(sub.clone());
        assert_eq!(got.first("chat.reset")["hasMore"], true);
        scroll_to_start(&again, &got, &sub).await;
        let all = items(&again);
        assert_eq!(nums(&all), (0..30).collect::<Vec<_>>());
        let seqs: Vec<i64> = all.as_array().unwrap().iter().map(|i| i["seq"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|p| p[1] > p[0]), "{seqs:?}");
        assert_eq!(seqs[0], -26);
        watchers().stop(&again.key);
    });
}
