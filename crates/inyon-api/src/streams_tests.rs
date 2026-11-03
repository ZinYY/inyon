//! The hub's side of `/ws2`: apps that come back with their epoch and seq, the quiet rule, the send
//! queue and compression. A chat's watcher is stood in for by feeding its messages to the stream,
//! and the pane isn't on the machine, so no real watcher starts.

use std::cell::RefCell;
use std::rc::Rc;

use inyon_chat_core::v2::{decode, Overview2};
use inyon_chat_core::{apply_splices, Chat};
use inyon_core::hosts::host::Host;
use inyon_core::hosts::registry::{registry, HostRuntime, RegistryEvent};
use inyon_protocol::{ChatInfo, ChatItem, HostState, LiveItem, ServerMsg, ServerMsg2, StreamPos};
use serde_json::{json, Value};

use super::*;
use crate::clients::{add, close_client, listen, new_client2, ClientRef};

const KEY: &str = "h:%9";

/// A `/ws2` app and everything the hub sent it.
struct App {
    c: ClientRef,
    got: Rc<RefCell<Vec<Wire>>>,
}

fn app() -> App {
    let got = Rc::new(RefCell::new(vec![]));
    let g = got.clone();
    let c = new_client2(move |w| g.borrow_mut().push(w));
    add(&c);
    App { c, got }
}

impl App {
    fn send(&self, v: Value) {
        client_message2(&self.c, &v.to_string());
    }

    fn follow(&self, stream: &str, at: Option<StreamPos>) {
        self.send(json!({ "t": "sub", "stream": stream, "at": at }));
    }

    /// The v2 frames since the last call.
    fn frames(&self) -> Vec<ServerMsg2> {
        self.got
            .borrow_mut()
            .drain(..)
            .filter_map(|w| match w {
                Wire::Text(t) => decode(Some(&t), None).ok(),
                Wire::Binary(b) => decode(None, Some(&b)).ok(),
            })
            .collect()
    }

    fn close(self) {
        close_client(&self.c);
    }
}

fn pos(f: &ServerMsg2) -> StreamPos {
    match f {
        ServerMsg2::Snapshot(s) => StreamPos { epoch: s.epoch, seq: s.seq },
        ServerMsg2::Delta(d) => StreamPos { epoch: d.epoch, seq: d.seq },
        _ => panic!("not a stream frame: {f:?}"),
    }
}

fn delta(f: &ServerMsg2) -> &inyon_protocol::Delta {
    match f {
        ServerMsg2::Delta(d) => d,
        _ => panic!("not a delta: {f:?}"),
    }
}

/// A machine "h" whose one pane (%7) runs Claude; chats in these tests are on %9, which it doesn't
/// have, so following one starts no watcher.
fn machine(state: &str, activity: &str) -> HostState {
    serde_json::from_value(json!({
        "config": { "id": "h", "name": "h", "kind": "ssh" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "work", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [
            { "id": "%7", "index": 0, "active": true, "command": "claude", "cwd": "/w", "title": "claude", "width": 100, "height": 30, "dead": false,
              "agent": { "kind": "claude", "pid": 1, "state": state, "activity": activity } }
        ] }] }],
    }))
    .unwrap()
}

fn with_machine() -> Rc<RefCell<HostRuntime>> {
    let state = machine("working", "Reading");
    let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state)));
    rt.borrow_mut().hold_polls();
    registry().insert_runtime("h", rt.clone());
    rt
}

fn info(more: Value) -> ChatInfo {
    let mut v = json!({ "hostId": "h", "paneId": "%9", "running": true, "transcriptPath": "/t.jsonl", "queue": [], "tasks": [], "subagents": [], "artifacts": [], "stats": { "turns": 0, "compactions": 0 } });
    if let (Value::Object(a), Value::Object(b)) = (&mut v, more) {
        a.extend(b);
    }
    serde_json::from_value(v).unwrap()
}

fn item(seq: i64, kind: &str, text: &str) -> ChatItem {
    let v = match kind {
        "user" => json!({ "kind": "user", "seq": seq, "uuid": format!("u{seq}"), "ts": "2026-10-31T12:00:00.000Z", "text": text, "images": 0 }),
        _ => {
            json!({ "kind": "assistant", "seq": seq, "uuid": format!("u{seq}"), "ts": "2026-10-31T12:00:00.000Z", "msgId": format!("m{seq}"), "block": { "type": "text", "text": text } })
        }
    };
    serde_json::from_value(v).unwrap()
}

fn transcript(n: i64) -> Vec<ChatItem> {
    (0..n).map(|i| item(i, if i % 2 == 0 { "user" } else { "assistant" }, &format!("message {i}: {}", "words ".repeat(20)))).collect()
}

fn reset(items: Vec<ChatItem>, has_more: bool) -> ServerMsg {
    ServerMsg::ChatReset { key: KEY.into(), info: Box::new(info(json!({}))), items, has_more }
}

fn live(id: &str, text: &str) -> ServerMsg {
    let items: Vec<LiveItem> = if text.is_empty() { vec![] } else { vec![serde_json::from_value(json!({ "id": id, "kind": "text", "text": text })).unwrap()] };
    ServerMsg::ChatLive { key: KEY.into(), items }
}

fn screen(working: bool) -> ServerMsg {
    ServerMsg::ChatScreen { key: KEY.into(), screen: Box::new(serde_json::from_value(json!({ "working": working, "width": 120 })).unwrap()) }
}

fn feed(msg: ServerMsg) {
    feed_for_test(KEY, &msg);
}

/// An app that follows the chat and has its first page.
fn following() -> (App, StreamPos) {
    let (a, first) = following_with_snapshot();
    (a, pos(&first))
}

fn following_with_snapshot() -> (App, ServerMsg2) {
    let a = app();
    a.follow(KEY, None);
    assert!(a.frames().is_empty(), "nothing to show before the watcher says anything");
    feed(reset(transcript(6), false));
    let mut f = a.frames();
    assert!(matches!(f[..], [ServerMsg2::Snapshot(_)]), "{f:?}");
    (a, f.remove(0))
}

#[test]
fn an_app_back_with_nothing_missed_gets_an_empty_delta_and_a_resent_page_sends_nothing() {
    crate::test_util::block(async {
        with_machine();
        let (a, at) = following();
        // the watcher sends its page again (an app subscribing, a restart from its saved copy)
        feed(reset(transcript(6), false));
        assert!(a.frames().is_empty(), "the same 6 messages: no frame at all");
        a.close();

        let b = app();
        b.follow(KEY, Some(at));
        let f = b.frames();
        assert_eq!(f.len(), 1);
        let d = delta(&f[0]);
        assert!(d.is_empty() && d.from == at.seq && d.seq == at.seq && d.epoch == at.epoch, "{d:?}");
        b.close();
    });
}

#[test]
fn an_app_back_after_a_small_gap_gets_one_delta_with_only_what_changed() {
    crate::test_util::block(async {
        with_machine();
        let (a, snapshot) = following_with_snapshot();
        let at = pos(&snapshot);
        let mut chat = Chat::new();
        let mut view = vec![];
        apply_splices(&mut view, &chat.apply2(&snapshot, 0).splices);
        a.close();

        // while it's away: a reply streams 40 times, lands in the record, and the agent stops
        let mut v1 = vec![reset(transcript(6), false)];
        let mut words = String::new();
        for i in 0..40 {
            words += &format!("word{i} ");
            v1.push(live("m6", &words));
        }
        v1.push(ServerMsg::ChatAppend { key: KEY.into(), items: vec![item(6, "assistant", &words)] });
        v1.push(live("m6", ""));
        v1.push(screen(false));
        for m in &v1[1..] {
            feed(m.clone());
        }

        let b = app();
        b.follow(KEY, Some(at));
        let f = b.frames();
        assert_eq!(f.len(), 1, "{f:?}");
        let d = delta(&f[0]);
        assert_eq!((d.epoch, d.from), (at.epoch, at.seq));
        assert!(d.seq > at.seq + 40);
        assert_eq!(d.append.as_ref().and_then(|r| r.items.as_ref()).map(Vec::len), Some(1));
        assert!(d.remove.is_none(), "the live reply came and went while it was away: nothing to remove");
        assert!(d.append.as_ref().is_some_and(|r| r.live.is_none()));
        assert!(d.state.as_ref().is_some_and(|s| s.screen.is_some() && s.info.is_none()));

        // and the app ends with the rows v1 messages give
        let applied = chat.apply2(&f[0], 0);
        assert!(applied.resync.is_none());
        apply_splices(&mut view, &applied.splices);
        let mut old = Chat::new();
        for m in &v1 {
            old.apply(m, 0);
        }
        assert_eq!(view, old.rows());
        b.close();
    });
}

#[test]
fn an_app_too_far_behind_gets_a_snapshot() {
    crate::test_util::block(async {
        set_gone_cap(3);
        with_machine();
        let (a, at) = following();
        a.close();
        // more live replies came and went than the stream remembers
        for i in 0..6 {
            feed(live(&format!("x{i}"), "typing"));
            feed(live(&format!("x{i}"), ""));
        }
        let b = app();
        b.follow(KEY, Some(at));
        let f = b.frames();
        assert!(matches!(&f[..], [ServerMsg2::Snapshot(s)] if s.epoch == at.epoch && s.seq > at.seq), "{f:?}");

        // one that left after the churn is still told in a delta
        let c = app();
        c.follow(KEY, Some(pos(&f[0])));
        feed(screen(true));
        assert!(matches!(&c.frames()[..], [ServerMsg2::Delta(d), ServerMsg2::Delta(e)] if d.is_empty() && e.state.is_some()));
        b.close();
        c.close();
    });
}

#[test]
fn a_rewritten_record_or_a_restarted_hub_starts_a_new_epoch() {
    crate::test_util::block(async {
        with_machine();
        let (a, at) = following();
        // compacted into a new record: the same places hold other messages
        let rewritten: Vec<ChatItem> = (0..3)
            .map(|i| {
                let mut it = item(i, "user", "summary");
                if let ChatItem::User { uuid, .. } = &mut it {
                    *uuid = format!("new-{uuid}");
                }
                it
            })
            .collect();
        feed(reset(rewritten.clone(), false));
        let f = a.frames();
        let [ServerMsg2::Snapshot(s)] = &f[..] else { panic!("{f:?}") };
        assert!(s.epoch > at.epoch, "a new epoch");
        assert_eq!(s.rows.items.as_ref().unwrap(), &rewritten);
        let after = pos(&f[0]);

        // an app still on the old epoch gets the snapshot too
        let b = app();
        b.follow(KEY, Some(at));
        assert!(matches!(&b.frames()[..], [ServerMsg2::Snapshot(s)] if s.epoch == after.epoch));

        // the hub restarts: its streams are gone, and the first one it makes is newer still
        a.close();
        b.close();
        forget_all();
        let c = app();
        c.follow(KEY, Some(after));
        assert!(c.frames().is_empty(), "nothing read yet");
        feed(reset(rewritten, false));
        assert!(matches!(&c.frames()[..], [ServerMsg2::Snapshot(s)] if s.epoch > after.epoch));
        c.close();
    });
}

#[test]
fn a_page_resent_with_one_message_grown_is_one_replace() {
    crate::test_util::block(async {
        with_machine();
        let a = app();
        a.follow(KEY, None);
        // OpenCode resends its latest page on every step, with has_more
        let mut page = transcript(8);
        feed(reset(page[2..].to_vec(), true));
        let first = a.frames();
        assert!(matches!(first[..], [ServerMsg2::Snapshot(_)]));
        page[7] = item(7, "assistant", "message 7, now longer");
        feed(reset(page[2..].to_vec(), true));
        let f = a.frames();
        let d = delta(&f[0]);
        assert_eq!(d.replace.as_ref().and_then(|r| r.items.clone()), Some(vec![page[7].clone()]));
        assert!(d.append.is_none() && d.remove.is_none());
        a.close();
    });
}

#[test]
fn a_quiet_app_hears_of_the_overview_only_when_an_agent_changes_state_and_catches_up_on_return() {
    crate::test_util::block(async {
        listen();
        let rt = with_machine();
        let changed = |state: &str, activity: &str| {
            rt.borrow_mut().state = machine(state, activity);
            registry().emit_for_test(RegistryEvent::Overview);
        };
        let a = app();
        a.follow(OVERVIEW, None);
        a.follow(KEY, None);
        feed(reset(transcript(2), false));
        let mut o = Overview2::default();
        for f in a.frames().iter().filter(|f| f.stream() == Some(OVERVIEW)) {
            o.apply(f);
        }
        assert_eq!(o.hosts.len(), 1);
        changed("working", "Thinking");
        inyon_core::platform::rt::sleep(400).await;
        let f = a.frames();
        assert!(matches!(&f[..], [ServerMsg2::Delta(d)] if d.replace.as_ref().is_some_and(|r| r.hosts.is_some())), "on screen, every change: {f:?}");
        o.apply(&f[0]);

        a.send(json!({ "t": "quiet", "on": true }));
        changed("working", "Editing");
        feed(ServerMsg::ChatAppend { key: KEY.into(), items: vec![item(2, "user", "more")] });
        inyon_core::platform::rt::sleep(400).await;
        assert!(a.frames().is_empty(), "the spinner moved and the chat grew: nothing yet");
        changed("waiting", "Editing");
        inyon_core::platform::rt::sleep(400).await;
        let f = a.frames();
        assert_eq!(f.len(), 1, "the agent asks something: the overview, nothing else: {f:?}");
        let (hosts, resync) = o.apply(&f[0]);
        assert!(resync.is_none() && hosts.is_some());
        changed("waiting", "Editing again");
        inyon_core::platform::rt::sleep(400).await;
        assert!(a.frames().is_empty());

        // back on screen: each stream up to date, in one delta each
        a.send(json!({ "t": "quiet", "on": false }));
        let f = a.frames();
        assert_eq!(f.iter().map(|f| f.stream().unwrap()).collect::<Vec<_>>(), [OVERVIEW, KEY], "{f:?}");
        assert!(f.iter().all(|f| matches!(f, ServerMsg2::Delta(_))));
        let (hosts, _) = o.apply(&f[0]);
        assert!(serde_json::to_string(&hosts.unwrap()).unwrap().contains("Editing again"));
        assert_eq!(delta(&f[1]).append.as_ref().and_then(|r| r.items.clone()), Some(vec![item(2, "user", "more")]));
        assert!(crate::clients::watched(), "an app on screen follows the overview");
        a.close();
    });
}

fn page(host: &str, slug: &str, done: u64) -> inyon_protocol::PageEntry {
    serde_json::from_value(
        json!({ "host": host, "slug": slug, "name": slug, "project": "/p", "by": "cli", "added": 1, "folder": format!("/p/{slug}"), "progress": { "done": done, "total": 9 } }),
    )
    .unwrap()
}

#[test]
fn the_pages_stream_is_read_when_followed_and_sends_only_the_pages_that_changed() {
    crate::test_util::block(async {
        let asked = Rc::new(std::cell::Cell::new(0));
        let n = asked.clone();
        on_pages_wanted(move || n.set(n.get() + 1));
        // nobody follows: the hub's reads go nowhere
        pages_changed(vec![page("local", "plan", 0)]);
        assert!(!pages_followed());

        let a = app();
        a.follow(PAGES, None);
        assert_eq!(asked.get(), 1, "following asks the hub to read its pages");
        assert!(a.frames().is_empty(), "nothing until the hub has read them");
        assert!(pages_followed());
        pages_changed(vec![page("local", "plan", 1), page("lap", "plan", 0)]);
        let f = a.frames();
        let ServerMsg2::Snapshot(snap) = &f[0] else { panic!("{f:?}") };
        assert_eq!(snap.rows.pages.as_ref().map(Vec::len), Some(2));
        let mut p = inyon_chat_core::v2::Pages2::default();
        p.apply(&f[0]);

        // a board moved on one machine: that page alone, in place
        pages_changed(vec![page("local", "plan", 2), page("lap", "plan", 0)]);
        let f = a.frames();
        assert_eq!(delta(&f[0]).replace.as_ref().and_then(|r| r.pages.clone()), Some(vec![page("local", "plan", 2)]));
        p.apply(&f[0]);
        // the same list again: nothing goes out
        pages_changed(vec![page("local", "plan", 2), page("lap", "plan", 0)]);
        assert!(a.frames().is_empty());

        // in the background nothing comes; back on screen, it's read again and caught up in one delta
        a.send(json!({ "t": "quiet", "on": true }));
        pages_changed(vec![page("local", "plan", 2)]);
        assert!(a.frames().is_empty(), "quiet");
        a.send(json!({ "t": "quiet", "on": false }));
        assert_eq!(asked.get(), 2, "coming back asks for a fresh read");
        let f = a.frames();
        assert_eq!(delta(&f[0]).remove.as_ref().and_then(|k| k.pages.clone()), Some(vec!["lap/plan".to_string()]));
        let (pages, resync) = p.apply(&f[0]);
        assert_eq!((pages, resync), (Some(vec![page("local", "plan", 2)]), None));

        // an app back after a reconnect with nothing missed gets the empty delta
        let at = Some(pos(&f[0]));
        let b = app();
        b.follow(PAGES, at);
        assert!(matches!(&b.frames()[..], [ServerMsg2::Delta(d)] if d.is_empty()));
        a.close();
        b.close();
        assert!(!pages_followed());
    });
}

#[test]
fn the_send_queue_is_the_hubs_and_every_app_sees_the_same_rows() {
    crate::test_util::block(async {
        with_machine();
        let (a, _) = following();
        let b = app();
        b.follow(KEY, None);
        b.frames();
        let queued = info(json!({
            "held": [{ "id": "h1", "text": "after this turn", "at": 5 }],
            "sends": [{ "id": "s1", "text": "from the phone", "at": 6, "state": "sending" }],
            "queue": ["typed in the terminal"],
        }));
        feed(ServerMsg::ChatInfo { key: KEY.into(), info: Box::new(queued.clone()) });
        let (fa, fb) = (a.frames(), b.frames());
        assert_eq!(fa, fb);
        let d = delta(&fa[0]);
        let rows = d.append.as_ref().and_then(|r| r.queue.clone()).unwrap();
        assert_eq!(rows.iter().map(inyon_protocol::QueuedMsg::key).collect::<Vec<_>>(), ["held:h1", "send:s1", "agent:0"]);
        assert!(d.state.is_none(), "the rest of the info didn't change, and it carries no queue");
        // the send goes through: one row replaced
        let mut sent = queued.clone();
        sent.sends.as_mut().unwrap()[0].state = inyon_protocol::SendStatus::Sent;
        feed(ServerMsg::ChatInfo { key: KEY.into(), info: Box::new(sent) });
        let d = a.frames();
        let r = delta(&d[0]).replace.as_ref().and_then(|r| r.queue.clone()).unwrap();
        assert_eq!((r.len(), r[0].state), (1, Some(inyon_protocol::SendStatus::Sent)));
        a.close();
        b.close();
    });
}

#[test]
fn the_app_that_typed_a_draft_isnt_sent_it_back() {
    crate::test_util::block(async {
        with_machine();
        let (a, _) = following();
        let b = app();
        b.follow(KEY, None);
        b.frames();
        let typed = ServerMsg::ChatDraft { key: KEY.into(), text: "half a thought".into(), source: inyon_protocol::DraftSource::App, at: 9 };
        chat_message(KEY, &typed, Some(&a.c));
        let (fa, fb) = (a.frames(), b.frames());
        assert!(delta(&fa[0]).is_empty(), "its seq moves on, without the text: {fa:?}");
        assert_eq!(delta(&fb[0]).state.as_ref().and_then(|s| s.draft.as_ref()).map(|d| d.text.as_str()), Some("half a thought"));
        assert_eq!(pos(&fa[0]), pos(&fb[0]));
        a.close();
        b.close();
    });
}

#[test]
fn large_frames_go_compressed_to_an_app_that_asked_and_small_ones_as_text() {
    crate::test_util::block(async {
        with_machine();
        let a = app();
        a.send(json!({ "t": "hello", "v": 2, "zip": ["deflate-raw"] }));
        a.follow(KEY, None);
        feed(reset(transcript(40), false));
        feed(screen(true));
        let got: Vec<Wire> = a.got.borrow_mut().drain(..).collect();
        assert!(matches!(&got[0], Wire::Text(t) if t == r#"{"t":"hello","v":2,"zip":"deflate-raw"}"#), "{got:?}");
        let Wire::Binary(zipped) = &got[1] else { panic!("the snapshot is large: {got:?}") };
        let json = String::from_utf8(miniz_oxide::inflate::decompress_to_vec(zipped).unwrap()).unwrap();
        assert!(json.starts_with(r#"{"t":"snapshot""#));
        assert!(zipped.len() * 4 < json.len(), "{} bytes as {}", json.len(), zipped.len());
        assert!(matches!(&got[2], Wire::Text(t) if t.starts_with(r#"{"t":"delta""#)), "a small delta stays text");

        // without hello, nothing is compressed
        let b = app();
        b.follow(KEY, None);
        assert!(b.got.borrow().iter().all(|w| matches!(w, Wire::Text(_))));
        a.close();
        b.close();
    });
}

#[test]
fn v1_stream_messages_are_ignored_on_ws2() {
    crate::test_util::block(async {
        with_machine();
        let a = app();
        a.send(json!({ "t": "sub.overview" }));
        a.send(json!({ "t": "sub.chat", "host": "h", "pane": "%9" }));
        assert!(a.got.borrow().is_empty());
        a.send(json!({ "t": "ping" }));
        assert!(matches!(&a.got.borrow()[..], [Wire::Text(t)] if t == r#"{"t":"pong"}"#));
        a.close();
    });
}

/// Bytes on the wire for a real conversation: `INYON_V2_TRANSCRIPT=<a Claude Code .jsonl>`.
#[test]
#[ignore]
fn measure_a_real_chat() {
    let path = std::env::var("INYON_V2_TRANSCRIPT").expect("INYON_V2_TRANSCRIPT");
    let text = std::fs::read_to_string(path).unwrap();
    let mut meta = inyon_core::claude::transcript::new_meta();
    let items: Vec<ChatItem> = inyon_core::claude::transcript::numbered(
        text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|e| inyon_core::claude::transcript::normalize(&e, &mut meta, false)).collect(),
    );
    let page = |n: usize| items[items.len().saturating_sub(n)..].to_vec();
    let v1_reset = json(&ServerMsg::ChatReset { key: KEY.into(), info: Box::new(info(json!({}))), items: page(160), has_more: true });
    let mut st = Stream::new(KEY);
    st.on_chat(&ServerMsg::ChatReset { key: KEY.into(), info: Box::new(info(json!({}))), items: page(160)[..page(160).len().saturating_sub(10)].to_vec(), has_more: true });
    let at = st.pos();
    st.on_chat(&ServerMsg::ChatReset { key: KEY.into(), info: Box::new(info(json!({}))), items: page(160), has_more: true });
    let gap = json(&st.catch_up(Some(at)).unwrap());
    let none = json(&st.catch_up(Some(st.pos())).unwrap_or(ServerMsg2::Delta(Delta {
        stream: KEY.into(),
        epoch: 0,
        from: 0,
        seq: 0,
        remove: None,
        replace: None,
        append: None,
        state: None,
    })));
    let snap = json(&ServerMsg2::Snapshot(st.snapshot()));
    let z = |s: &str| deflate(s.as_bytes()).len();
    let t = std::time::Instant::now();
    for _ in 0..20 {
        deflate(snap.as_bytes());
    }
    let per = t.elapsed() / 20;
    for level in [1, 9] {
        let t = std::time::Instant::now();
        let n = miniz_oxide::deflate::compress_to_vec(snap.as_bytes(), level).len();
        println!("  at level {level}: {n} bytes, {:?}", t.elapsed());
    }
    println!("items in the conversation: {}", items.len());
    println!("v1 chat.reset (last 160): {} bytes", v1_reset.len());
    println!("v2 snapshot (same 160): {} bytes, {} compressed ({:.1}x), {per:?} to compress", snap.len(), z(&snap), snap.len() as f64 / z(&snap) as f64);
    println!("v2 back with nothing missed: {} bytes", none.len());
    println!("v2 back after the last 10 messages: {} bytes, {} compressed", gap.len(), z(&gap));
    // a reply streaming in: each tick one live row replaced
    let reply: String = items
        .iter()
        .rev()
        .find_map(|i| match i {
            ChatItem::Assistant { block: inyon_protocol::AssistantBlock::Text { text }, .. } if text.len() > 2000 => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "word ".repeat(600));
    let (mut raw, mut zipped, mut v1_live) = (0, 0, 0);
    let ticks: Vec<usize> = (1..=20).map(|i| reply.len() * i / 20).collect();
    let mut st = Stream::new(KEY);
    st.on_chat(&ServerMsg::ChatInfo { key: KEY.into(), info: Box::new(info(json!({ "transcriptPath": "" }))) });
    for end in ticks {
        let end = (0..=end).rev().find(|&e| reply.is_char_boundary(e)).unwrap();
        let msg = live("m1", &reply[..end]);
        v1_live += json(&msg).len();
        if let Out::Delta(d) = st.on_chat(&msg) {
            let s = json(&ServerMsg2::Delta(*d));
            raw += s.len();
            zipped += if s.len() >= ZIP_MIN { z(&s) } else { s.len() };
        }
    }
    println!("a {}-byte reply in 20 ticks: v1 chat.live {v1_live} bytes, v2 deltas {raw}, compressed {zipped}", reply.len());
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}
