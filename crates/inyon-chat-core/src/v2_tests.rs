use inyon_protocol::{ChatInfo, ChatItem, ChatState, Delta, Keys, LiveItem, QueuedMsg, Rows, ServerMsg, ServerMsg2, Snapshot, StreamPos};
use serde_json::{json, Value};

use super::*;
use crate::chat_list::{apply_splices, ListRow};
use crate::test_support::{chat_info, host, item, pane, T0};

const NOW: u64 = T0 as u64;
const K: &str = "box:%1";

fn snap(epoch: u64, seq: u64, items: Vec<ChatItem>, live: Vec<LiveItem>, queue: Vec<QueuedMsg>, info: ChatInfo, loaded: bool) -> ServerMsg2 {
    let (info, _) = inyon_protocol::v2::split_queue(&info);
    ServerMsg2::Snapshot(Snapshot {
        stream: K.into(),
        epoch,
        seq,
        rows: Rows { items: Some(items), live: Some(live), queue: Some(queue), ..Rows::default() },
        state: Some(ChatState { info: Some(Box::new(info)), has_more: Some(false), loaded: Some(loaded), ..ChatState::default() }),
    })
}

fn delta(epoch: u64, from: u64, seq: u64) -> Delta {
    Delta { stream: K.into(), epoch, from, seq, remove: None, replace: None, append: None, state: None }
}

fn appended(epoch: u64, from: u64, items: Vec<ChatItem>) -> ServerMsg2 {
    ServerMsg2::Delta(Delta { append: Some(Rows { items: Some(items), ..Rows::default() }), ..delta(epoch, from, from + 1) })
}

fn live(id: &str, text: &str) -> LiveItem {
    serde_json::from_value(json!({ "id": id, "kind": "text", "text": text })).unwrap()
}

/// Applies frames and checks that the splices keep a copy of the list in step with the chat's own.
fn run(chat: &mut Chat, view: &mut Vec<ListRow>, frames: &[ServerMsg2]) -> Vec<Option<ClientMsg2>> {
    frames
        .iter()
        .map(|f| {
            let a = chat.apply2(f, NOW);
            apply_splices(view, &a.splices);
            assert_eq!(view, chat.rows());
            a.resync
        })
        .collect()
}

/// The same chat built from v1 messages.
fn v1(msgs: &[ServerMsg]) -> Vec<ListRow> {
    let mut c = Chat::new();
    for m in msgs {
        c.apply(m, NOW);
    }
    c.rows().to_vec()
}

fn reset_v1(items: Vec<ChatItem>, info: ChatInfo) -> ServerMsg {
    ServerMsg::ChatReset { key: K.into(), info: Box::new(info), items, has_more: false }
}

#[test]
fn a_snapshot_and_deltas_build_the_rows_v1_messages_build() {
    let (a, b, c) = (item::user("fix the build"), item::text("Looking."), item::end(1200.0));
    let info = chat_info(json!({}));
    let mut chat = Chat::new();
    let mut view = vec![];
    let resyncs = run(
        &mut chat,
        &mut view,
        &[
            snap(7, 1, vec![a.clone()], vec![], vec![], info.clone(), true),
            ServerMsg2::Delta(Delta { append: Some(Rows { live: Some(vec![live("m1", "Look")]), ..Rows::default() }), ..delta(7, 1, 2) }),
            ServerMsg2::Delta(Delta { replace: Some(Rows { live: Some(vec![live("m1", "Looking.")]), ..Rows::default() }), ..delta(7, 2, 3) }),
            // the record lands and the live text goes in the same delta
            ServerMsg2::Delta(Delta {
                append: Some(Rows { items: Some(vec![b.clone(), c.clone()]), ..Rows::default() }),
                remove: Some(Keys { live: Some(vec!["m1".into()]), ..Keys::default() }),
                ..delta(7, 3, 4)
            }),
        ],
    );
    assert!(resyncs.iter().all(Option::is_none));
    assert_eq!(chat.position(), Some(StreamPos { epoch: 7, seq: 4 }));
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![a.clone()], info), ServerMsg::ChatAppend { key: K.into(), items: vec![b, c] }]));
    assert!(chat.live.as_ref().is_some_and(Vec::is_empty));
}

#[test]
fn a_reconnect_with_a_gap_catches_up_with_one_delta() {
    let (a, b, c, d) = (item::user("one"), item::text("two"), item::user("three"), item::text("four"));
    let info = chat_info(json!({}));
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![a.clone()], vec![], vec![], info.clone(), true), appended(7, 1, vec![b.clone()])]);
    // away while seqs 3 to 9 went by; on its return the hub sends what changed since 2, as one delta
    let resync = run(
        &mut chat,
        &mut view,
        &[ServerMsg2::Delta(Delta {
            append: Some(Rows { items: Some(vec![c.clone(), d.clone()]), ..Rows::default() }),
            state: Some(ChatState { screen: Some(Box::new(serde_json::from_value(json!({ "working": false, "width": 80 })).unwrap())), ..ChatState::default() }),
            ..delta(7, 2, 9)
        })],
    );
    assert_eq!(resync, [None]);
    assert_eq!(chat.position(), Some(StreamPos { epoch: 7, seq: 9 }));
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![a, b, c, d], info)]));
}

#[test]
fn a_new_epoch_asks_for_a_resync_and_its_snapshot_replaces_everything() {
    let (a, b) = (item::user("before the rewrite"), item::text("old"));
    let info = chat_info(json!({}));
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![a, b], vec![], vec![], info.clone(), true)]);
    // a delta from an epoch this chat never saw can't be applied
    let resync = run(&mut chat, &mut view, &[appended(8, 4, vec![item::user("lost")])]);
    assert_eq!(resync, [Some(ClientMsg2::Sub { stream: K.into(), at: Some(StreamPos { epoch: 7, seq: 1 }) })]);
    // asked once, not on every frame after it
    assert_eq!(run(&mut chat, &mut view, &[appended(8, 5, vec![item::user("lost too")])]), [None]);
    let c = item::user("after the rewrite");
    run(&mut chat, &mut view, &[snap(8, 6, vec![c.clone()], vec![], vec![], info.clone(), true)]);
    assert_eq!(chat.position(), Some(StreamPos { epoch: 8, seq: 6 }));
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![c], info)]));
}

#[test]
fn frames_out_of_order_wait_for_the_one_before_them_and_duplicates_are_dropped() {
    let (a, b, c, d) = (item::user("one"), item::text("two"), item::end(5.0), item::user("three"));
    let info = chat_info(json!({}));
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![a.clone()], vec![], vec![], info.clone(), true)]);
    let (f2, f3, f4) = (appended(7, 1, vec![b.clone()]), appended(7, 2, vec![c.clone()]), appended(7, 3, vec![d.clone()]));
    let resync = run(&mut chat, &mut view, &[f4.clone(), f3.clone()]);
    assert!(resync[0].is_some(), "a gap: the hub is asked, in case the missing frame never comes");
    assert_eq!(chat.position(), Some(StreamPos { epoch: 7, seq: 1 }));
    run(&mut chat, &mut view, std::slice::from_ref(&f2));
    assert_eq!(chat.position(), Some(StreamPos { epoch: 7, seq: 4 }), "the held ones follow it");
    // the same frames again (the old socket's last ones), and the hub's answer to the resync
    let before = chat.rows().to_vec();
    for f in [f2, f3, f4, snap(7, 3, vec![a.clone(), b.clone(), c.clone()], vec![], vec![], info.clone(), true)] {
        let applied = chat.apply2(&f, NOW);
        assert_eq!(applied, Applied::default());
    }
    assert_eq!(chat.rows(), before);
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![a, b, c, d], info)]));
}

#[test]
fn a_catch_up_that_starts_before_the_chat_is_asked_for_again() {
    let info = chat_info(json!({}));
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![item::user("one")], vec![], vec![], info, true), appended(7, 1, vec![item::text("two")]), appended(7, 2, vec![item::end(1.0)])]);
    // from 2 to 9 can't be applied on top of 3 in general (rows may have moved since 2)
    let resync = run(
        &mut chat,
        &mut view,
        &[appended(7, 2, vec![item::user("x")])].map(|f| match f {
            ServerMsg2::Delta(d) => ServerMsg2::Delta(Delta { seq: 9, ..d }),
            f => f,
        }),
    );
    assert_eq!(resync, [Some(ClientMsg2::Sub { stream: K.into(), at: Some(StreamPos { epoch: 7, seq: 3 }) })]);
}

#[test]
fn the_send_queue_rows_show_as_the_same_rows_v1_info_does() {
    let info = chat_info(json!({
        "queue": ["typed in the terminal"],
        "held": [{ "id": "h1", "text": "after this turn", "at": NOW }],
        "sends": [{ "id": "s1", "text": "from the phone", "at": NOW, "state": "sent" }],
    }));
    let (_, queue) = inyon_protocol::v2::split_queue(&info);
    assert_eq!(queue.iter().map(QueuedMsg::key).collect::<Vec<_>>(), ["held:h1", "send:s1", "agent:0"]);
    let a = item::user("hi");
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![a.clone()], vec![], queue.clone(), info.clone(), true)]);
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![a.clone()], info.clone())]));
    // another device's send is delivered: the hub replaces its row, and the bubble goes
    let mut delivered = queue[1].clone();
    delivered.state = Some(inyon_protocol::SendStatus::Delivered);
    delivered.uuid = Some("u-sent".into());
    run(&mut chat, &mut view, &[ServerMsg2::Delta(Delta { replace: Some(Rows { queue: Some(vec![delivered]), ..Rows::default() }), ..delta(7, 1, 2) })]);
    let mut after = info.clone();
    after.sends.as_mut().unwrap()[0].state = inyon_protocol::SendStatus::Delivered;
    after.sends.as_mut().unwrap()[0].uuid = Some("u-sent".into());
    assert_eq!(chat.rows(), v1(&[reset_v1(vec![a], after)]));
    assert!(!chat.rows().iter().any(|r| matches!(r, ListRow::Pending(p) if p.text == "from the phone")));
}

#[test]
fn a_snapshot_before_the_record_is_read_keeps_what_the_chat_showed() {
    let a = item::user("kept from before");
    let info = chat_info(json!({ "transcriptPath": "/t.jsonl" }));
    let mut chat = Chat::new();
    let mut view = vec![];
    run(&mut chat, &mut view, &[snap(7, 1, vec![a.clone()], vec![], vec![], info.clone(), true)]);
    // the hub restarted: its first snapshot has the title but not the messages yet
    run(&mut chat, &mut view, &[snap(9, 1, vec![], vec![], vec![], chat_info(json!({ "transcriptPath": "/t.jsonl", "title": "Build" })), false)]);
    assert_eq!(chat.items, vec![a]);
    assert_eq!(chat.info.as_ref().and_then(|i| i.title.as_deref()), Some("Build"));
}

#[test]
fn a_compressed_frame_decodes_and_a_v1_message_comes_back_as_text() {
    let frame = appended(7, 1, vec![item::text(&"a long reply ".repeat(200))]);
    let json = serde_json::to_string(&frame).unwrap();
    let zipped = miniz_oxide::deflate::compress_to_vec(json.as_bytes(), 6);
    assert!(zipped.len() * 5 < json.len());
    assert_eq!(decode(None, Some(&zipped)), Ok(frame));
    let notify = r#"{"t":"notify","host":"box","pane":"%1","kind":"done","title":"Claude","body":"Done"}"#;
    assert_eq!(decode(Some(notify), None), Err(Some(notify.to_owned())));
    assert_eq!(decode(None, Some(b"not deflate at all")), Err(None));
}

#[test]
fn the_overview_stream_hands_over_the_machines_when_they_change() {
    let box1 = host("box", vec![pane("%1", json!({}))], json!({}));
    let lap = host("laptop", vec![], json!({}));
    let mut o = Overview2::default();
    let snapshot = ServerMsg2::Snapshot(Snapshot { stream: "overview".into(), epoch: 3, seq: 1, rows: Rows { hosts: Some(vec![box1.clone()]), ..Rows::default() }, state: None });
    assert_eq!(o.apply(&snapshot), (Some(vec![box1.clone()]), None));
    let mut busy = box1.clone();
    busy.latency_ms = Some(40);
    let d = |from, seq, v: Value| -> ServerMsg2 {
        let mut d: Delta = serde_json::from_value(v).unwrap();
        (d.from, d.seq) = (from, seq);
        ServerMsg2::Delta(d)
    };
    let changed = d(1, 2, json!({ "stream": "overview", "epoch": 3, "from": 0, "seq": 0, "replace": { "hosts": [busy] }, "append": { "hosts": [lap] } }));
    assert_eq!(o.apply(&changed).0, Some(vec![busy.clone(), lap.clone()]));
    assert_eq!(o.apply(&changed), (None, None), "a duplicate");
    let gone = d(2, 3, json!({ "stream": "overview", "epoch": 3, "from": 0, "seq": 0, "remove": { "hosts": ["box"] } }));
    assert_eq!(o.apply(&gone).0, Some(vec![lap]));
}

#[test]
fn the_organize_stream_hands_over_the_marks_when_they_change() {
    let mark = |id: &str, pinned: u32| -> inyon_protocol::SessionMark {
        serde_json::from_value(json!({ "id": id, "host": "box", "pane": "%1", "live": true, "pinned": pinned })).unwrap()
    };
    let mut o = Organize2::default();
    let snapshot =
        ServerMsg2::Snapshot(Snapshot { stream: "organize".into(), epoch: 5, seq: 1, rows: Rows { sessions: Some(vec![mark("s-a", 0)]), ..Rows::default() }, state: None });
    let (org, resync) = o.apply(&snapshot);
    assert_eq!((org.unwrap().sessions, resync), (vec![mark("s-a", 0)], None));
    let d = |from: u64, seq: u64, v: Value| -> ServerMsg2 {
        let mut d: Delta = serde_json::from_value(v).unwrap();
        (d.from, d.seq) = (from, seq);
        ServerMsg2::Delta(d)
    };
    let pinned = d(
        1,
        2,
        json!({ "stream": "organize", "epoch": 5, "from": 0, "seq": 0, "replace": { "sessions": [mark("s-a", 1)] }, "append": { "sessions": [mark("s-b", 0)], "projects": [{ "id": "p-1", "name": "Inyon" }] } }),
    );
    let org = o.apply(&pinned).0.unwrap();
    assert_eq!((org.sessions, org.projects.len()), (vec![mark("s-a", 1), mark("s-b", 0)], 1));
    // a delta from a later epoch: ask again from where it is
    let other = d(2, 3, json!({ "stream": "organize", "epoch": 6, "from": 0, "seq": 0, "remove": { "sessions": ["s-a"] } }));
    assert_eq!(o.apply(&other), (None, Some(ClientMsg2::Sub { stream: "organize".into(), at: Some(StreamPos { epoch: 5, seq: 2 }) })));
}

#[test]
fn the_pages_stream_keeps_every_page_by_machine_and_slug() {
    let page = |host: &str, slug: &str, done: u64| -> inyon_protocol::PageEntry {
        serde_json::from_value(
            json!({ "host": host, "slug": slug, "name": slug, "project": "/p", "by": "cli", "added": 1, "folder": "/p/plan", "progress": { "done": done, "total": 9 } }),
        )
        .unwrap()
    };
    let mut p = Pages2::default();
    let snapshot = ServerMsg2::Snapshot(Snapshot {
        stream: "pages".into(),
        epoch: 7,
        seq: 1,
        rows: Rows { pages: Some(vec![page("local", "plan", 1), page("lap", "plan", 0)]), ..Rows::default() },
        state: None,
    });
    let (pages, resync) = p.apply(&snapshot);
    assert_eq!((pages.map(|l| l.len()), resync), (Some(2), None));
    let d = |from: u64, seq: u64, v: Value| -> ServerMsg2 {
        let mut d: Delta = serde_json::from_value(v).unwrap();
        (d.from, d.seq) = (from, seq);
        ServerMsg2::Delta(d)
    };
    // the same slug on two machines are two pages: a board that moved replaces only its own
    let moved = d(1, 2, json!({ "stream": "pages", "epoch": 7, "from": 0, "seq": 0, "replace": { "pages": [page("local", "plan", 2)] } }));
    assert_eq!(p.apply(&moved).0, Some(vec![page("local", "plan", 2), page("lap", "plan", 0)]));
    let gone = d(2, 3, json!({ "stream": "pages", "epoch": 7, "from": 0, "seq": 0, "remove": { "pages": ["lap/plan"] } }));
    assert_eq!(p.apply(&gone).0, Some(vec![page("local", "plan", 2)]));
    assert_eq!(p.apply(&gone), (None, None), "a duplicate");
    let early = d(5, 6, json!({ "stream": "pages", "epoch": 7, "from": 0, "seq": 0 }));
    assert_eq!(p.apply(&early).1, Some(ClientMsg2::Sub { stream: "pages".into(), at: Some(StreamPos { epoch: 7, seq: 3 }) }));
}
