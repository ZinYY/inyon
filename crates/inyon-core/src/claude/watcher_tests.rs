//! claude/draft-sent.test.ts and claude/watcher-limits.test.ts. The TS spies on the registry; here
//! the machine is put in this thread's registry instead.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::json;

use super::*;
use crate::claude::transcript::Base;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::rt::{run, wait};
use crate::test_support::{assert_match, machine};

/// A subscriber that keeps what it's sent.
#[derive(Default)]
struct Phone {
    got: RefCell<Vec<ServerMsg>>,
}

impl Subscriber for Phone {
    fn send(&self, msg: &ServerMsg) {
        self.got.borrow_mut().push(msg.clone());
    }
}

impl Phone {
    fn drafts(&self) -> Vec<(String, DraftSource)> {
        self.got
            .borrow()
            .iter()
            .filter_map(|m| match m {
                ServerMsg::ChatDraft { text, source, .. } => Some((text.clone(), *source)),
                _ => None,
            })
            .collect()
    }
}

const KEY: &str = "draft-test:%9";

fn draft_box() -> (Rc<Fake>, Rc<ChatWatcher>) {
    let fake = Fake::install(|_, _, _| ok(""));
    machine("draft-test", "local", json!([{ "id": "%9", "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
    (fake, watchers().get("draft-test", "%9"))
}

fn draft(text: &str, at: f64, mirrored: Option<&str>) -> Option<Draft> {
    Some(Draft { text: text.into(), source: DraftSource::App, at, mirrored: mirrored.map(str::to_owned) })
}

// draft-sent.test.ts

#[test]
fn a_phone_keyboard_putting_a_sent_message_back_in_the_box_doesnt_make_it_a_draft_again() {
    run(|| async {
        let (_fake, w) = draft_box();
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        w.subscribe(sub.clone());

        w.draft_from_app("I see only problem when I send", Some(&sub));
        w.draft_sent(Some("I see only problem when I send"), now_ms() - 1000);
        assert_eq!(get_draft(KEY), None);

        // a moment later the keyboard puts the message (or its last word) back, and the app passes it on
        phone.got.borrow_mut().clear();
        w.draft_from_app("I see only problem when I send", Some(&sub));
        w.draft_from_app("send", Some(&sub));
        assert_eq!(get_draft(KEY), None);
        // and that app is told the box is empty, so its own copy clears too
        assert_eq!(phone.drafts().into_iter().map(|d| d.0).collect::<Vec<_>>(), ["", ""]);

        // a new message is a draft as usual
        w.draft_from_app("and another thing", Some(&sub));
        assert_eq!(get_draft(KEY).map(|d| d.text).as_deref(), Some("and another thing"));
        watchers().stop(KEY);
    });
}

#[test]
fn finishing_a_send_keeps_a_new_draft_typed_meanwhile_even_one_that_starts_the_same_way() {
    run(|| async {
        let (_fake, w) = draft_box();
        let started_at = now_ms() - 2000;
        set_draft(KEY, draft("Fix", (started_at + 1) as f64, None));
        w.draft_sent(Some("Fix the login bug"), started_at);
        assert_eq!(get_draft(KEY).map(|d| d.text).as_deref(), Some("Fix"));
        // but the message itself, pushed just before the send and arriving after it began, goes
        set_draft(KEY, draft("Fix the login bug", (started_at + 1) as f64, None));
        w.draft_sent(Some("Fix the login bug\n\nAttached files:\n/x/a.png"), started_at);
        assert_eq!(get_draft(KEY), None);
        watchers().stop(KEY);
    });
}

#[test]
fn emptying_the_box_isnt_undone_by_a_screen_read_from_before_the_terminal_caught_up() {
    run(|| async {
        let (_fake, w) = draft_box();
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        set_draft(KEY, draft("old text here", now_ms() as f64, Some("old text here")));
        let read = |d: Option<&str>| {
            // a busy agent redraws all the time, so the screen is read again before the box is cleared
            let mut screen = blank_screen();
            screen.working = true;
            screen.width = 120;
            screen.input_visible = Some(true);
            screen.draft = d.map(str::to_owned);
            {
                let mut st = w.st.borrow_mut();
                st.screen = screen;
                st.mirroring_until = 0;
                st.sending_until = 0;
            }
            w.sync_draft_from_screen();
        };
        w.draft_from_app("", None);
        read(Some("old text here"));
        assert_eq!(get_draft(KEY).map(|d| d.text).unwrap_or_default(), "");
        assert!(phone.drafts().iter().all(|d| d.1 != DraftSource::Terminal));
        // once the terminal's box is empty too, nothing is left over
        read(None);
        assert_eq!(get_draft(KEY), None);
        // and typing at the terminal is still picked up
        read(Some("typed on the laptop"));
        assert_match(&get_draft(KEY), json!({ "text": "typed on the laptop", "source": "terminal" }));
        watchers().stop(KEY);
    });
}

// watcher-limits.test.ts

#[test]
fn in_the_background_an_open_chat_reads_its_screen_rarely_and_again_at_once_on_return() {
    run(|| async {
        let reads = Rc::new(Cell::new(0));
        let r = reads.clone();
        let _fake = Fake::install(move |_, cmd, _| {
            if cmd.contains("capture-pane") {
                r.set(r.get() + 1);
            }
            ok("")
        });
        machine("quiet-test", "local", json!([{ "id": "%7", "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
        watchers().get("quiet-test", "%7");
        wait(2300).await;
        // in the foreground it's every 2 s
        assert!(reads.get() >= 2, "{}", reads.get());

        watchers().quiet(true);
        wait(100).await;
        reads.set(0);
        wait(2300).await;
        assert_eq!(reads.get(), 0);

        watchers().quiet(false);
        wait(100).await;
        assert_eq!(reads.get(), 1);
        watchers().stop("quiet-test:%7");
    });
}

#[test]
fn a_very_long_gemini_or_antigravity_chat_keeps_only_its_latest_messages_like_the_others() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        machine("quiet-test", "local", json!([{ "id": "%7", "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
        let w = watchers().get("quiet-test", "%7");
        let base = Base::new("u", "t");
        let items: Vec<ChatItem> = (0..5000).map(|i| with_seq(base.with_uuid(format!("u{i}")).user(format!("message {i}"), 0), i)).collect();
        w.publish_snapshot(items);
        let held = w.items();
        assert_eq!(held.len(), 4000);
        assert!(matches!(held.last(), Some(ChatItem::User { text, .. }) if text == "message 4999"));
        watchers().stop("quiet-test:%7");
    });
}

// new-agent.test.ts

const NEW_KEY: &str = "new-agent-test:%5";

/// Claude as it is right after starting: no transcript until something is said.
fn new_agent_box(pid: u32, started_at: u64) {
    machine(
        "new-agent-test",
        "local",
        json!([{ "id": "%5", "title": "claude", "width": 100, "agent": { "kind": "claude", "pid": pid, "startedAt": started_at, "state": "idle" } }]),
    )
    .borrow_mut()
    .hold_polls();
}

fn said() -> Vec<ChatItem> {
    let base = Base::new("u", "t");
    vec![
        with_seq(base.with_uuid("u1").user("Hello", 0), 1),
        with_seq(base.with_uuid("a1").assistant("m1", None, inyon_protocol::AssistantBlock::Text { text: "Hi Abhash, what are you working on today?".into() }), 2),
    ]
}

/// A chat on this pane showing the first Claude's conversation.
fn chat_of_first() -> Rc<ChatWatcher> {
    new_agent_box(100, 1_000);
    let w = watchers().get("new-agent-test", "%5");
    {
        let mut st = w.st.borrow_mut();
        st.transcript = Some("/home/abhash/.claude/projects/x/old.jsonl".into());
        st.proc = Some("100:1000".into());
        st.reader = Some(crate::claude::transcript_tail::TranscriptReader::new(400));
        st.older_from = Some(0);
        st.items = said();
        st.ready = true;
    }
    w
}

fn texts(w: &ChatWatcher) -> Vec<String> {
    w.items()
        .iter()
        .filter_map(|i| match i {
            ChatItem::User { text, .. } => Some(text.clone()),
            ChatItem::Assistant { block: inyon_protocol::AssistantBlock::Text { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn fresh_new_agent() {
    watchers().stop(NEW_KEY);
    WATCHERS.with(|w| w.parked.borrow_mut().shift_remove(NEW_KEY));
}

#[test]
fn a_chat_left_open_while_claude_exits_and_starts_again_empties_instead_of_showing_the_old_conversation() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        fresh_new_agent();
        let w = chat_of_first();
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        // the same Claude, its transcript not known for a moment: nothing changes
        w.sync();
        assert_eq!(texts(&w), ["Hello", "Hi Abhash, what are you working on today?"]);
        // a new Claude in the pane, nothing said to it yet
        new_agent_box(200, 2_000);
        phone.got.borrow_mut().clear();
        w.sync();
        assert!(texts(&w).is_empty());
        let resets: Vec<usize> = phone.got.borrow().iter().filter_map(|m| if let ServerMsg::ChatReset { items, .. } = m { Some(items.len()) } else { None }).collect();
        assert_eq!(resets, [0]);
        watchers().stop(NEW_KEY);
    });
}

#[test]
fn reopening_the_chat_later_shows_the_new_claude_empty_from_the_saved_copy_as_from_a_recently_closed_one() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        for from in ["recently closed", "saved copy"] {
            fresh_new_agent();
            chat_of_first().save();
            watchers().stop(NEW_KEY);
            if from == "saved copy" {
                WATCHERS.with(|w| w.parked.borrow_mut().shift_remove(NEW_KEY));
            }
            new_agent_box(200, 2_000);
            assert_eq!((from, texts(&watchers().get("new-agent-test", "%5"))), (from, vec![]));
            watchers().stop(NEW_KEY);
        }
    });
}

#[test]
fn the_same_claude_reopened_still_shows_its_conversation() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        for from in ["recently closed", "saved copy"] {
            fresh_new_agent();
            chat_of_first().save();
            watchers().stop(NEW_KEY);
            if from == "saved copy" {
                WATCHERS.with(|w| w.parked.borrow_mut().shift_remove(NEW_KEY));
            }
            new_agent_box(100, 1_000);
            assert_eq!((from, texts(&watchers().get("new-agent-test", "%5"))), (from, vec!["Hello".to_owned(), "Hi Abhash, what are you working on today?".to_owned()]));
            watchers().stop(NEW_KEY);
        }
    });
}

// ---------- review ----------

#[test]
fn a_tail_that_keeps_ending_waits_at_most_30_seconds_however_often_it_ends() {
    assert_eq!([0, 1, 4, 5, 60, u32::MAX].map(retry_ms), [1000, 2000, 16_000, 30_000, 30_000, 30_000]);
}

#[test]
fn a_clock_that_ticks_between_two_reads_never_trips_up_the_draft_mirror() {
    run(|| async {
        let (_fake, w) = draft_box();
        let t = now_ms();
        w.st.borrow_mut().sending_until = t + 1;
        // every read is 2 ms on from the last: the send window is open at the first read, over at a second
        let tick = Rc::new(std::cell::Cell::new(t));
        let k = tick.clone();
        crate::clock::set_source(Some(Rc::new(move || {
            let v = k.get();
            k.set(v + 2);
            v
        })));
        w.mirror();
        crate::clock::set_source(None);
        watchers().stop(KEY);
    });
}

// live replies read off the screen

fn live_of(phone: &Phone) -> Option<Vec<inyon_protocol::LiveItem>> {
    phone.got.borrow().iter().rev().find_map(|m| match m {
        ServerMsg::ChatLive { items, .. } => Some(items.clone()),
        _ => None,
    })
}

#[test]
fn a_reply_claude_writes_shows_live_until_its_record_lands_and_doesnt_come_back() {
    run(|| async {
        let screen = crate::test_support::fixture("screens/claude/reply-streaming.txt");
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %7\n{screen}")) } else { ok("") });
        machine("live-test", "local", json!([{ "id": "%7", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        let w = watchers().get("live-test", "%7");
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        w.subscribe(sub);
        w.poll_screen().await;
        wait(200).await;
        let live = live_of(&phone).expect("live text");
        assert_eq!(live.len(), 1);
        assert!(live[0].id.starts_with("screen:"), "{}", live[0].id);
        assert!(live[0].text.starts_with("Tea likely began in southwestern China"), "{}", live[0].text);

        // its record lands: the live copy goes
        let base = Base::new("u", "t");
        let text = "Tea likely began in southwestern China, where people chewed or boiled wild leaves. A legend credits the mythical emperor Shennong.";
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("a9").assistant("m9", None, inyon_protocol::AssistantBlock::Text { text: text.into() }), 9));
        assert!(w.settle_screen_live());
        w.live_soon();
        wait(200).await;
        assert_eq!(live_of(&phone), Some(vec![]));

        // the screen still shows it, but it's on record now: it stays gone
        let sent = phone.got.borrow().len();
        w.poll_screen().await;
        wait(200).await;
        assert!(phone.got.borrow()[sent..].iter().all(|m| !matches!(m, ServerMsg::ChatLive { items, .. } if !items.is_empty())));
        watchers().stop("live-test:%7");
    });
}

#[test]
fn an_earlier_turns_record_landing_late_sends_the_reply_being_written_again() {
    run(|| async {
        let screen = crate::test_support::fixture("screens/claude/reply-streaming.txt");
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %9\n{screen}")) } else { ok("") });
        machine("live-late", "local", json!([{ "id": "%9", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        let w = watchers().get("live-late", "%9");
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        w.subscribe(sub);
        w.poll_screen().await;
        wait(200).await;
        let live = live_of(&phone).expect("live text");

        // the turn before's reply is recorded only now: the app drops the screen's copy on it, so
        // the hub sends that copy again, unchanged
        let sent = phone.got.borrow().len();
        let base = Base::new("u", "t");
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("a8").assistant("m8", None, inyon_protocol::AssistantBlock::Text { text: "Coffee came from Ethiopia.".into() }), 8));
        assert!(w.settle_screen_live());
        w.live_soon();
        wait(200).await;
        let again = phone.got.borrow()[sent..].iter().rev().find_map(|m| match m {
            ServerMsg::ChatLive { items, .. } => Some(items.clone()),
            _ => None,
        });
        assert_eq!(again, Some(live));
        watchers().stop("live-late:%9");
    });
}

#[test]
fn nothing_is_live_while_the_agent_isnt_working() {
    run(|| async {
        // the finished reply, with no spinner: Claude is idle
        let screen = crate::test_support::fixture("screens/claude/reply-long.txt");
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %8\n{screen}")) } else { ok("") });
        machine("live-idle", "local", json!([{ "id": "%8", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
        let w = watchers().get("live-idle", "%8");
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        w.subscribe(sub);
        w.poll_screen().await;
        wait(200).await;
        assert!(live_of(&phone).is_none_or(|l| l.is_empty()));
        watchers().stop("live-idle:%8");
    });
}

#[test]
fn after_its_turn_the_screens_copy_waits_for_the_record_then_goes_on_its_own() {
    run(|| async {
        let screen = Rc::new(RefCell::new(crate::test_support::fixture("screens/claude/reply-streaming.txt")));
        let s = screen.clone();
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %6\n{}", s.borrow())) } else { ok("") });
        machine("live-wait", "local", json!([{ "id": "%6", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        let w = watchers().get("live-wait", "%6");
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        w.subscribe(sub);
        w.poll_screen().await;
        wait(200).await;
        assert!(live_of(&phone).is_some_and(|l| l.len() == 1));

        // the turn ends (no spinner) before the record lands: the reply stays meanwhile
        *screen.borrow_mut() = crate::test_support::fixture("screens/claude/reply-long.txt");
        w.poll_screen().await;
        wait(200).await;
        assert!(live_of(&phone).is_some_and(|l| l.len() == 1), "kept until its record");
        // the record comes in a snapshot, as Antigravity's and Gemini's do
        let base = Base::new("u", "t");
        let text = "Tea likely began in southwestern China, where people chewed or boiled wild leaves. A legend credits Shennong.";
        w.publish_snapshot(vec![with_seq(base.with_uuid("a1").assistant("m1", None, inyon_protocol::AssistantBlock::Text { text: text.into() }), 1)]);
        wait(200).await;
        assert_eq!(live_of(&phone), Some(vec![]));

        // a reply whose record never comes goes after a while
        *screen.borrow_mut() = crate::test_support::fixture("screens/claude/reply-streaming.txt");
        w.st.borrow_mut().items.clear();
        w.poll_screen().await;
        wait(200).await;
        assert!(live_of(&phone).is_some_and(|l| l.len() == 1));
        *screen.borrow_mut() = crate::test_support::fixture("screens/claude/reply-long.txt");
        w.poll_screen().await;
        wait(SCREEN_LIVE_WAIT_MS + 300).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        watchers().stop("live-wait:%6");
    });
}
