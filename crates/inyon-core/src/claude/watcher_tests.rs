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

#[test]
fn bursts_of_terminal_output_do_not_overlap_screen_reads() {
    run(|| async {
        let (fake, w) = draft_box();
        wait(30).await;
        fake.calls.borrow_mut().clear();
        futures_util::future::join_all([w.poll_screen(), w.poll_screen(), w.poll_screen()]).await;
        assert_eq!(fake.cmds().iter().filter(|s| s.contains("capture-pane")).count(), 1);
        assert!(w.st.borrow().screen_read.is_none());
        w.stop();
    });
}

#[test]
fn native_claude_stream_is_independent_of_screen_polls_and_rejects_old_session_frames() {
    run(|| async {
        use inyon_protocol::claude_stream::{DisplayEvent, DisplaySnapshot};
        let (fake, w) = draft_box();
        let id = "11111111-1111-4111-8111-111111111111";
        let next_id = "22222222-2222-4222-8222-222222222222";
        let now = now_ms();
        let ts = date::to_iso((now - 1000) as f64).unwrap();
        {
            let mut st = w.st.borrow_mut();
            st.info.session_id = Some(id.into());
            st.screen.working = true;
            st.items = vec![with_seq(Base::new("prompt", &ts).user("聊聊吉他", 0), 1)];
        }
        let raw = |sid: &str, text: &str| {
            let event: DisplayEvent = serde_json::from_value(json!({"hook_event_name":"MessageDisplay","session_id":sid,"turn_id":sid,"message_id":sid,"index":0,"delta":text,"final":false})).unwrap();
            serde_json::to_string(&DisplaySnapshot::apply(None, event, now).unwrap()).unwrap()
        };
        w.start_display(id, now - 2000);
        let first = fake.streams.borrow().last().unwrap().clone();
        assert!(first.cmd.contains("claude-display watch"));
        w.start_display(id, now - 2000);
        assert_eq!(fake.streams.borrow().len(), 1, "reuse the existing reader");
        first.line(&raw(id, "连续输出 🎸"));
        assert_eq!(w.st.borrow().screen_live.as_ref().unwrap().text, "连续输出 🎸");
        first.line("{}");
        assert!(w.st.borrow().screen_live.is_some(), "heartbeat is not a clear");
        first.line("null");
        assert!(w.st.borrow().screen_live.is_none());
        w.st.borrow_mut().screen.working = false;
        first.line(&raw(id, "withdrawn reply"));
        assert!(w.st.borrow().screen_live.is_none(), "an idle rewound session cannot replay its old cache");
        w.st.borrow_mut().screen.working = true;
        {
            let mut st = w.st.borrow_mut();
            st.generation += 1;
            st.info.session_id = Some(next_id.into());
        }
        w.start_display(next_id, now - 2000);
        assert!(first.killed.get());
        first.line(&raw(id, "old reply"));
        assert!(w.st.borrow().screen_live.is_none(), "late output from before resume is ignored");
        let second = fake.streams.borrow().last().unwrap().clone();
        second.line(&raw(next_id, "new reply"));
        assert_eq!(w.st.borrow().screen_live.as_ref().unwrap().text, "new reply");
        w.stop();
        assert!(second.killed.get());
        second.line(&raw(next_id, "after stop"));
        assert!(w.st.borrow().screen_live.is_none());
    });
}

#[test]
fn confirmed_claude_controls_are_bound_to_the_current_transcript() {
    run(|| async {
        let (_fake, w) = draft_box();
        {
            let mut st = w.st.borrow_mut();
            st.transcript = Some("/current.jsonl".into());
            super::super::controls::record(&mut st.meta.claude_controls, "Set effort level to low", 2000);
        }
        assert_eq!(w.claude_status("/current.jsonl", None).unwrap().effort.as_deref(), Some("low"));
        assert!(w.claude_status("/resumed.jsonl", None).is_none());
        w.st.borrow_mut().meta = new_meta();
        assert!(w.claude_status("/current.jsonl", None).is_none());
        watchers().stop(KEY);
    });
}

#[test]
fn rename_confirmation_uses_the_record_for_this_session_not_a_pending_ui_title() {
    run(|| async {
        let (_fake, w) = draft_box();
        {
            let mut st = w.st.borrow_mut();
            st.info.session_id = Some("current".into());
            st.info.title = Some("pending rename".into());
        }
        assert_eq!(w.recorded_title("current"), None);
        {
            let mut st = w.st.borrow_mut();
            st.meta.title = Some("automatic title".into());
            st.meta.custom_title = Some("随便聊聊".into());
        }
        assert_eq!(w.recorded_title("current").as_deref(), Some("随便聊聊"));
        assert_eq!(w.recorded_title("another-window"), None);
        watchers().stop(KEY);
    });
}

#[test]
fn manual_reload_discards_only_the_chat_cache_and_keeps_subscribers_and_drafts() {
    run(|| async {
        let fake = Fake::install(|_, cmd, _| {
            if cmd == size_command("/tmp/existing-conversation.jsonl", 0) {
                return ok(&format!("{} 1", TAIL_BYTES * 4));
            }
            ok("")
        });
        machine("reload-test", "local", json!([{ "id": "%9", "agent": { "kind": "claude", "pid": 7, "state": "working", "sessionId":"existing-conversation", "transcriptPath":"/tmp/existing-conversation.jsonl" } }])).borrow_mut().hold_polls();
        let old = watchers().get("reload-test", "%9");
        {
            let mut st = old.st.borrow_mut();
            st.transcript = Some("/tmp/existing-conversation.jsonl".into());
            st.reader = Some(TranscriptReader::new(100));
            st.items = said();
            st.older_from = Some(0);
            st.ready = true;
        }
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        old.subscribe(sub.clone());
        old.track_send("in-flight", "/resume another-session", false);
        old.track_send("confirmed", "/model opus", true);
        old.settle_send("confirmed", Some(SendStatus::Delivered), None);
        old.save();
        set_draft("reload-test:%9", draft("unsent work", now_ms() as f64, None));
        assert!(load_chat("reload-test:%9").is_some());
        watchers().reload("reload-test", "%9").unwrap();
        let fresh = watchers().get("reload-test", "%9");
        assert!(!Rc::ptr_eq(&old, &fresh));
        assert!(old.st.borrow().stopped);
        assert_eq!(fresh.subscribers(), 1);
        assert_eq!(fresh.delivery("in-flight").unwrap().state, SendStatus::Sending);
        assert_eq!(fresh.delivery("confirmed").unwrap().state, SendStatus::Delivered);
        fresh.settle_send("in-flight", Some(SendStatus::Delivered), None);
        assert_eq!(fresh.delivery("in-flight").unwrap().state, SendStatus::Delivered);
        assert!(load_chat("reload-test:%9").is_none());
        assert_eq!(get_draft("reload-test:%9").unwrap().text, "unsent work");
        wait(20).await;
        // Check the actual read boundary, not the one-shot flag consumed when follow starts.
        assert_eq!(fresh.st.borrow().reader.as_ref().unwrap().offset, 0);
        assert!(fake.streams.borrow().iter().any(|s| !s.killed.get() && s.cmd == tail_command("/tmp/existing-conversation.jsonl", 0, false, true)));
        phone.got.borrow_mut().clear();
        old.more(&sub, 10).await;
        assert!(phone.got.borrow().is_empty(), "a retired reader must not republish stale pages");
        assert!(fake.cmds().iter().all(|c| !c.contains("send-keys") && !c.contains("paste-buffer") && !c.contains("kill-pane")));
        watchers().stop("reload-test:%9");
    });
}

#[test]
fn manual_reload_rejects_an_unreadable_pane_without_discarding_its_cached_chat() {
    run(|| async {
        let (_fake, old) = draft_box();
        old.st.borrow_mut().items = said();
        assert!(watchers().reload("draft-test", "%9").is_err());
        assert!(Rc::ptr_eq(&old, &watchers().get("draft-test", "%9")));
        assert_eq!(texts(&old), ["Hello", "Hi Abhash, what are you working on today?"]);
        watchers().stop(KEY);
    });
}

#[test]
fn cached_codex_segments_keep_their_earlier_pages_after_restart_without_duplicate_items() {
    run(|| async {
        let path = "/home/test/.codex/sessions/current.jsonl";
        let previous = "/home/test/.codex/sessions/rollout-00000000-0000-0000-0000-000000000001.jsonl";
        let record = |n: usize| json!({"timestamp":"2026-11-03T00:00:00Z","type":"response_item","payload":{"id":format!("record-{n}"),"type":"message","role":"user","content":[{"type":"input_text","text":format!("message {n}")}]}});
        let history = (0..360).map(|n| stringify(&record(n)) + "\n").collect::<String>();
        let header = stringify(&json!({"type":"session_meta","payload":{"history_base":{"thread_id":"00000000-0000-0000-0000-000000000001","end_byte_offset":history.len()}}}));
        let root_header = stringify(&record(0));
        let _fake = Fake::install(move |_, command, _| {
            if command == crate::agents::codex::codex_history_head_command(path) { return ok(&header); }
            if command.starts_with("find ") { return ok(previous); }
            if command == crate::agents::codex::codex_history_head_command(previous) { return ok(&root_header); }
            if command.starts_with("head -c ") { return ok(&history); }
            ok("")
        });
        machine("history-cache-test", "local", json!([{ "id": "%9", "agent": { "kind": "codex", "pid": 7, "state": "idle", "sessionId":"current" } }])).borrow_mut().hold_polls();
        let w = watchers().get("history-cache-test", "%9");
        let mut meta = new_meta();
        let items = (350..=360).flat_map(|n| normalize_codex(&record(n), &mut meta)).enumerate().map(|(n, item)| with_seq(item, 1000 + n as i64)).collect();
        {
            let mut st = w.st.borrow_mut();
            st.transcript = Some(path.into());
            st.info.session_id = Some("current".into());
            st.reader = Some(TranscriptReader::new(100));
            st.codex = true;
            st.agent = Some(AgentKind::Codex);
            st.items = items;
            st.seq = 1011;
            st.older_from = Some(0);
            st.ready = true;
        }
        // A hub restart loses held_history, but the disk cache must still advertise earlier pages.
        w.save();
        w.restart();
        w.load_cached();
        assert!(w.older_in_file());
        assert!(w.st.borrow().held_history.is_none());
        let phone = Rc::new(Phone::default());
        let sub: Sub = phone.clone();
        let mut before = w.items()[0].seq();
        loop {
            w.more(&sub, before).await;
            let message = phone.got.borrow().last().cloned().unwrap();
            let ServerMsg::ChatMore { items, has_more, .. } = message else { panic!("history page expected") };
            assert!(!items.is_empty());
            before = items[0].seq();
            if !has_more { break; }
        }
        assert_eq!(texts(&w), (0..=360).map(|n| format!("message {n}")).collect::<Vec<_>>());
        assert!(!w.older_in_file());
        // In-memory parking also loses the raw history; resolving it again adds no duplicates.
        let parked = w.park().unwrap();
        w.restart();
        w.unpark(parked);
        w.more(&sub, w.items()[0].seq()).await;
        assert_eq!(w.items().len(), 361);
        assert!(!w.older_in_file());
        // If the cache cut falls among restored items without known file positions,
        // rebuild it on reopening instead of treating its artificial seq as a file index.
        w.save();
        assert!(load_chat("history-cache-test:%9").unwrap().at.unwrap().older.is_none());
        w.restart();
        w.load_cached();
        assert!(w.st.borrow().reader.is_none());
        watchers().stop("history-cache-test:%9");
    });
}

fn draft_box() -> (Rc<Fake>, Rc<ChatWatcher>) {
    let fake = Fake::install(|_, _, _| ok(""));
    machine("draft-test", "local", json!([{ "id": "%9", "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
    (fake, watchers().get("draft-test", "%9"))
}

#[test]
fn claude_rewind_waits_through_an_empty_frame_for_the_restored_prompt() {
    run(|| async {
        let lines = Rc::new(RefCell::new("Confirm you want to restore the conversation to the point before this message:\n│ Explain training and test sets.\n│ (1 minute ago)\n❯ 1. Restore conversation\n".to_owned()));
        let current = lines.clone();
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %9\n{}", current.borrow())) } else { ok("") });
        machine("delayed-rewind", "local", json!([{ "id":"%9", "width":100, "agent":{"kind":"claude","pid":7,"startedAt":0,"state":"idle","sessionId":"session","transcriptPath":"/history.jsonl"} }])).borrow_mut().hold_polls();
        let w = watchers().get("delayed-rewind", "%9");
        {
            let mut st = w.st.borrow_mut();
            st.transcript = Some("/history.jsonl".into());
            st.proc = Some("7:0".into());
            st.items = vec![with_seq(Base::new("question", "t").user("Explain training and test sets.", 0), 0)];
        }
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        w.read_screen().await.unwrap();
        assert!(w.st.borrow().claude_rewind.is_some());
        // A fast terminal confirmation can disappear between screen polls. The
        // control connection's pre-Enter snapshot must drive the same selection.
        w.st.borrow_mut().claude_rewind = None;
        w.observe_terminal_confirmation(&lines.borrow().lines().map(str::to_owned).collect::<Vec<_>>());
        assert!(w.st.borrow().claude_rewind.is_some());
        *lines.borrow_mut() = format!("{}\n❯ \n{}\n", "─".repeat(100), "─".repeat(100));
        w.read_screen().await.unwrap();
        assert!(w.st.borrow().claude_rewind.is_some(), "the empty redraw is not cancellation");
        phone.got.borrow_mut().clear();
        *lines.borrow_mut() = format!("{}\n❯ Explain training and test sets.\n{}\n", "─".repeat(100), "─".repeat(100));
        w.read_screen().await.unwrap();
        assert!(super::super::branch::selection("delayed-rewind:%9", "/history.jsonl", Some("7:0"), 0.0).is_some());
        let got = phone.got.borrow();
        let restored = got.iter().position(|m| matches!(m, ServerMsg::ChatDraft { text, source: DraftSource::Terminal, .. } if text == "Explain training and test sets.")).unwrap();
        let reset = got.iter().position(|m| matches!(m, ServerMsg::ChatReset { .. })).unwrap();
        assert!(restored < reset, "the restored draft must arrive before the composer remounts");
        watchers().stop("delayed-rewind:%9");
    });
}

fn draft(text: &str, at: f64, mirrored: Option<&str>) -> Option<Draft> {
    Some(Draft { text: text.into(), source: DraftSource::App, at, mirrored: mirrored.map(str::to_owned) })
}

#[test]
fn slow_draft_writes_are_serial_and_partial_echo_cannot_replace_the_new_draft() {
    run(|| async {
        let (_fake, w) = draft_box();
        let writing = Deferred::new();
        {
            let mut st = w.st.borrow_mut();
            st.mirroring = Some(writing.clone());
            st.screen.input_visible = Some(true);
            st.screen.draft = Some("old partial echo".into());
            st.mirroring_until = 0;
        }
        set_draft(KEY, draft("latest full draft", now_ms() as f64, None));
        w.mirror();
        assert!(w.st.borrow().mirroring.as_ref().unwrap().same(&writing));
        assert!(w.st.borrow().mirror_again);
        w.sync_draft_from_screen();
        assert_eq!(get_draft(KEY).unwrap().text, "latest full draft");
        // Send must still wait for the original writer, even after another edit requested a mirror.
        let began = crate::platform::rt::spawn_task({ let w = w.clone(); async move { w.begin_send().await } });
        assert!(!began.is_settled());
        writing.resolve(());
        began.wait().await;
        assert!(!w.st.borrow().mirror_again);
        watchers().stop(KEY);
    });
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
    registry().update_pane("new-agent-test", "%5", |p| {
        let a = p.agent.as_mut().unwrap();
        a.session_id = Some("old".into());
        a.transcript_path = Some("/home/abhash/.claude/projects/x/old.jsonl".into());
    });
    {
        let mut st = w.st.borrow_mut();
        st.transcript = Some("/home/abhash/.claude/projects/x/old.jsonl".into());
        st.info.session_id = Some("old".into());
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
        // The same process without a current ID must no longer keep its previous chat.
        registry().update_pane("new-agent-test", "%5", |p| {
            let a = p.agent.as_mut().unwrap();
            a.session_id = None;
            a.transcript_path = None;
        });
        w.sync();
        assert!(texts(&w).is_empty());
        assert!(w.info().session_id.is_none());
        assert!(w.info().transcript_path.is_none());
        // a new Claude in the pane, nothing said to it yet
        new_agent_box(200, 2_000);
        phone.got.borrow_mut().clear();
        w.sync();
        assert!(texts(&w).is_empty());
        let resets: Vec<usize> = phone.got.borrow().iter().filter_map(|m| if let ServerMsg::ChatReset { items, .. } = m { Some(items.len()) } else { None }).collect();
        assert!(resets.is_empty(), "already empty while waiting for identity");
        watchers().stop(NEW_KEY);
    });
}

#[test]
fn clear_in_the_same_claude_process_empties_chat_before_reading_the_new_file() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        fresh_new_agent();
        let w = chat_of_first();
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        w.st.borrow_mut().info.session_id = Some("old-session".into());
        registry().update_pane("new-agent-test", "%5", |p| {
            let a = p.agent.as_mut().unwrap();
            a.session_id = Some("fresh-session".into());
            a.transcript_path = Some(r"C:\Users\user\.claude\projects\work\fresh-session.jsonl".into());
        });
        // Merely being offline is not evidence that the visible conversation should disappear.
        registry().get("new-agent-test").unwrap().borrow_mut().state.status = HostStatus::Connecting;
        w.sync();
        assert_eq!(texts(&w).len(), 2);
        registry().get("new-agent-test").unwrap().borrow_mut().state.status = HostStatus::Online;
        phone.got.borrow_mut().clear();
        w.sync();
        // No async file read has run yet. The empty reset must already have reached clients.
        assert!(texts(&w).is_empty());
        assert!(phone.got.borrow().iter().any(|m| matches!(m, ServerMsg::ChatReset { items, .. } if items.is_empty())));
        assert_eq!(w.info().session_id.as_deref(), Some("fresh-session"));
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
            registry().update_pane("new-agent-test", "%5", |p| {
                let a = p.agent.as_mut().unwrap();
                a.session_id = Some("old".into());
                a.transcript_path = Some("/home/abhash/.claude/projects/x/old.jsonl".into());
            });
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
fn codex_screen_text_grows_before_the_record_and_exact_deltas_take_priority() {
    run(|| async {
        let frame = |text: &str| format!("› Explain streaming\n\n• {text}\n\n  1 background terminal running · /ps to view · /stop to close\n\n› Ask Codex to do anything\n\n  GPT-6-Astra xhigh · ~/project\n  tab to queue message");
        let screen = Rc::new(RefCell::new(frame("第一部分正在生成")));
        let s = screen.clone();
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %7\n{}", s.borrow())) } else { ok("") });
        machine("codex-screen-live", "local", json!([{ "id":"%7", "width":120, "agent":{"kind":"codex","pid":7,"state":"working"} }])).borrow_mut().hold_polls();
        let w = watchers().get("codex-screen-live", "%7");
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        w.poll_screen().await;
        wait(100).await;
        let first = live_of(&phone).unwrap();
        assert_eq!(first[0].text, "第一部分正在生成");
        assert!(w.items().is_empty(), "not waiting for a completed transcript record");
        *screen.borrow_mut() = frame("第一部分正在生成，第二部分也出现了。");
        // An open working chat polls soon enough to show incremental output automatically.
        wait(600).await;
        let next = live_of(&phone).unwrap();
        assert_eq!(next[0].id, first[0].id);
        assert_eq!(next[0].text, "第一部分正在生成，第二部分也出现了。");
        w.on_live_line(r#"{"m":"item/agentMessage/delta","p":{"itemId":"exact","delta":"Exact daemon text"}}"#);
        wait(100).await;
        let exact = live_of(&phone).unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].text, "Exact daemon text");
        w.st.borrow_mut().live.clear();
        let base = Base::new("u", "t");
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("done").assistant("m", None, inyon_protocol::AssistantBlock::Text { text: next[0].text.clone() }), 1));
        w.settle_screen_live();
        w.live_soon();
        wait(100).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        w.poll_screen().await;
        wait(100).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        watchers().stop("codex-screen-live:%7");
    });
}

#[test]
fn codex_current_transcript_turn_streams_without_spinner_or_queue_hint() {
    run(|| async {
        let frame = |text: &str| format!("› Explain photography\n\n• {text}\n\n› Ask Codex to do anything\n\n  GPT-6-Astra high · ~/project · Read Only · weekly 27% left\n  ? for shortcuts");
        let captured = Rc::new(RefCell::new(frame("先理解光线")));
        let s = captured.clone();
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %7\n{}", s.borrow())) } else { ok("") });
        machine("codex-turn-live", "local", json!([{ "id":"%7", "width":120, "agent":{"kind":"codex","pid":7,"startedAt":1000,"state":"idle"} }])).borrow_mut().hold_polls();
        let w = watchers().get("codex-turn-live", "%7");
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        // An unfinished old turn from a resumed/forked history is not current activity.
        w.st.borrow_mut().meta.turn = Some(serde_json::from_value(json!({"id":"old","startedAt":500})).unwrap());
        w.poll_screen().await;
        wait(100).await;
        assert!(live_of(&phone).is_none_or(|items| items.is_empty()));
        assert!(!w.screen().working);
        w.st.borrow_mut().meta.turn = Some(serde_json::from_value(json!({"id":"now","startedAt":2000})).unwrap());
        w.poll_screen().await;
        wait(100).await;
        let first = live_of(&phone).unwrap();
        assert_eq!(first[0].text, "先理解光线");
        assert!(w.screen().working);
        *captured.borrow_mut() = frame("先理解光线，再考虑构图。");
        w.poll_screen().await;
        wait(100).await;
        assert_eq!(live_of(&phone).unwrap()[0].text, "先理解光线，再考虑构图。");
        let base = Base::new("u", "t");
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("done").assistant("m", None, inyon_protocol::AssistantBlock::Text { text: "先理解光线，再考虑构图。".into() }), 1));
        w.st.borrow_mut().meta.turn = None;
        w.settle_screen_live();
        w.live_soon();
        wait(100).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        watchers().stop("codex-turn-live:%7");
    });
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
fn claude_keeps_streaming_when_its_spinner_disappears_during_text_output() {
    run(|| async {
        let frame = Rc::new(RefCell::new(crate::test_support::fixture("screens/claude/reply-thinking.txt")));
        let captured = frame.clone();
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %7\n{}", captured.borrow())) } else { ok("") });
        machine("claude-hidden-spinner", "local", json!([{ "id": "%7", "width": 200, "agent": { "kind": "claude", "pid": 7, "state": "working" } }])).borrow_mut().hold_polls();
        let w = watchers().get("claude-hidden-spinner", "%7");
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        w.poll_screen().await;
        assert!(w.screen().working);

        *frame.borrow_mut() = crate::test_support::fixture("screens/claude/reply-streaming-macos.txt");
        w.poll_screen().await;
        wait(200).await;
        let first = live_of(&phone).expect("partial text before the transcript is written");
        assert_eq!(first.len(), 1);
        assert!(first[0].text.starts_with("欠拟合——只学会了"));

        let longer = frame.borrow().replace("盐 3.2 克、火候中偏大 17", "盐 3.2 克、火候中偏大 17 秒，换一口锅就不灵了。");
        *frame.borrow_mut() = longer;
        w.poll_screen().await;
        wait(200).await;
        let next = live_of(&phone).unwrap();
        assert!(next[0].text.starts_with(&first[0].text));
        assert!(next[0].text.ends_with("换一口锅就不灵了。"));
        assert_eq!(next[0].id, first[0].id);

        // Long answers scroll both the prompt and the assistant marker out of view.
        // Their remaining unmarked paragraphs must extend the preview, not erase it.
        let tail = next[0].text.split("\n\n").last().unwrap();
        *frame.borrow_mut() = format!("  {tail}\n\n  换了食材也能做好，才说明掌握了规律。\n\n────────────────────\n❯\n────────────────────");
        w.poll_screen().await;
        wait(200).await;
        let next = live_of(&phone).unwrap();
        assert_eq!(next[0].id, first[0].id);
        assert!(next[0].text.starts_with(&first[0].text), "{}", next[0].text);
        assert!(next[0].text.ends_with("换了食材也能做好，才说明掌握了规律。"));

        let base = Base::new("u", "t");
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("done").assistant("m", None, inyon_protocol::AssistantBlock::Text { text: next[0].text.clone() }), 1));
        w.settle_screen_live();
        w.live_soon();
        wait(200).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        w.poll_screen().await;
        wait(200).await;
        assert_eq!(live_of(&phone), Some(vec![]));
        watchers().stop("claude-hidden-spinner:%7");
    });
}

#[test]
fn windows_herdr_chat_waits_for_recorded_blocks_instead_of_screen_previews() {
    run(|| async {
        let screen = crate::test_support::fixture("screens/claude/reply-streaming.txt");
        let _fake = Fake::install(move |_, cmd, _| if cmd.contains("capture-pane") { ok(&format!("@@CAP %7\n{screen}")) } else { ok("") });
        machine("herdr-no-live", "local", json!([{ "id":"%7", "width":100, "agent":{"kind":"claude","pid":7,"state":"working"} }])).borrow_mut().hold_polls();
        registry().host("herdr-no-live").unwrap().update_config(|c| c.ssh_mode = Some("windows-herdr".into()));
        let w = watchers().get("herdr-no-live", "%7");
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        w.poll_screen().await;
        wait(200).await;
        assert!(!w.reads_live_reply(AgentKind::Claude));
        assert!(live_of(&phone).unwrap_or_default().is_empty());
        assert!(w.st.borrow().screen_live.is_none());
        watchers().stop("herdr-no-live:%7");
    });
}


#[test]
fn idle_windows_claude_does_not_launch_preview_readers() {
    run(|| async {
        let screen = crate::test_support::fixture("screens/claude/reply-long.txt");
        let _fake = Fake::install(move |_, cmd, _| {
            assert!(!cmd.contains("claude-display read "), "idle panes must not add polling processes");
            if cmd.contains("capture-pane") { ok(&format!("@@CAP %8.10\n{screen}")) } else { ok("") }
        });
        let rt = machine("windows-display-idle", "local", json!([{ "id":"%8.10", "width":120,"agent":{"kind":"claude","pid":7,"state":"idle","sessionId":"11111111-1111-4111-8111-111111111111","startedAt":1000} }]));
        rt.borrow_mut().hold_polls(); rt.borrow_mut().state.os = Some("Windows x64".into());
        let w = watchers().get("windows-display-idle", "%8.10");
        w.poll_screen().await;
        watchers().stop("windows-display-idle:%8.10");
    });
}

#[test]
fn windows_claude_streams_official_batches_while_its_terminal_only_shows_a_spinner() {
    run(|| async {
        use inyon_protocol::claude_stream::{DisplayEvent,DisplaySnapshot};
        let id="11111111-1111-4111-8111-111111111111";
        let now=now_ms();
        let event=|index,delta:&str| DisplayEvent { hook_event_name:"MessageDisplay".into(),session_id:id.into(),turn_id:id.into(),message_id:id.into(),index,delta:delta.into(),finished:false };
        let snapshot=Rc::new(RefCell::new(DisplaySnapshot::apply(None,event(0,"第一段 🎸\n"),now).unwrap()));
        let captured=snapshot.clone();
        let screen=crate::test_support::fixture("screens/claude/reply-thinking.txt");
        let _fake=Fake::install(move |_,cmd,_| {
            if cmd.contains("capture-pane") { ok(&format!("@@CAP %8.10\n{screen}")) }
            else if cmd.contains("claude-display read ") { ok(&serde_json::to_string(&*captured.borrow()).unwrap()) }
            else { ok("") }
        });
        let rt=machine("windows-display","local",json!([{"id":"%8.10","width":120,"agent":{"kind":"claude","pid":7,"state":"working","sessionId":id,"startedAt":now-2000}}]));
        rt.borrow_mut().hold_polls();
        rt.borrow_mut().state.os=Some("Windows x64".into());
        let w=watchers().get("windows-display","%8.10");
        let phone=Rc::new(Phone::default());w.subscribe(phone.clone());
        w.st.borrow_mut().info.session_id=Some(id.into());
        w.st.borrow_mut().items.push(ChatItem::User { seq:1,uuid:"u".into(),ts:crate::js::date::to_iso((now-1000) as f64).unwrap(),text:"练琴".into(),images:0,queued:None,shell:None });
        w.poll_screen().await;wait(100).await;
        assert_eq!(live_of(&phone).unwrap()[0].text,"第一段 🎸\n");
        let before=snapshot.borrow().clone();
        *snapshot.borrow_mut()=DisplaySnapshot::apply(Some(before),event(1,"第二段"),now+1).unwrap();
        w.poll_screen().await;wait(100).await;
        assert_eq!(live_of(&phone).unwrap()[0].text,"第一段 🎸\n第二段");
        let base=Base::new("u","t");
        w.st.borrow_mut().items.push(with_seq(base.with_uuid("done").assistant("api-message",None,inyon_protocol::AssistantBlock::Text{text:"第一段 🎸\n第二段".into()}),2));
        w.settle_screen_live();w.live_soon();wait(100).await;
        assert_eq!(live_of(&phone),Some(vec![]));
        w.poll_screen().await;wait(100).await;
        assert_eq!(live_of(&phone),Some(vec![]));
        watchers().stop("windows-display:%8.10");
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
        let text = live_of(&phone).unwrap()[0].text.clone();
        w.publish_snapshot(vec![with_seq(base.with_uuid("a1").assistant("m1", None, inyon_protocol::AssistantBlock::Text { text }), 1)]);
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

fn blank_codex_same_process() {
    let rt = machine(
        "new-agent-test", "local",
        json!([{ "id": "%5", "title": "work", "width": 100, "agent": { "kind": "codex", "pid": 100, "startedAt": 1000, "state": "idle" } }]),
    );
    let mut r = rt.borrow_mut();
    r.hold_polls();
    r.empty_codex.insert("%5".into());
}

#[test]
fn a_blank_codex_thread_discards_old_chat_even_when_the_process_is_unchanged() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        fresh_new_agent();
        let w = chat_of_first();
        w.st.borrow_mut().info.session_id = Some("other-panes-thread".into());
        let phone = Rc::new(Phone::default());
        w.subscribe(phone.clone());
        blank_codex_same_process();
        phone.got.borrow_mut().clear();
        w.sync();
        assert!(texts(&w).is_empty());
        assert!(w.info().session_id.is_none());
        assert!(w.info().transcript_path.is_none());
        assert!(phone.got.borrow().iter().any(|m| matches!(m, ServerMsg::ChatReset { items, .. } if items.is_empty())));
        watchers().stop(NEW_KEY);
    });
}

#[test]
fn a_known_blank_codex_id_discards_the_previous_threads_items_before_its_first_file() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        fresh_new_agent();
        let w = chat_of_first();
        let old = "00000000-0000-0000-0000-000000000001";
        let new = "00000000-0000-0000-0000-000000000002";
        w.st.borrow_mut().transcript = Some(format!("/sessions/rollout-{old}.jsonl"));
        // A screen update can publish the new ID before the watcher syncs.
        w.st.borrow_mut().info.session_id = Some(new.into());
        machine("new-agent-test", "local", json!([{ "id":"%5", "agent":{
            "kind":"codex", "pid":100, "startedAt":1000, "state":"idle", "sessionId":new
        }}])).borrow_mut().hold_polls();
        w.sync();
        assert!(texts(&w).is_empty());
        assert_eq!(w.info().session_id.as_deref(), Some(new));
        assert!(w.info().transcript_path.is_none());
        assert!(w.st.borrow().ready);
        watchers().stop(NEW_KEY);
    });
}

#[test]
fn reopening_a_blank_codex_thread_does_not_revive_parked_or_persisted_history() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        for from in ["recently closed", "saved copy"] {
            fresh_new_agent();
            chat_of_first().save();
            watchers().stop(NEW_KEY);
            if from == "saved copy" { WATCHERS.with(|w| w.parked.borrow_mut().shift_remove(NEW_KEY)); }
            blank_codex_same_process();
            assert_eq!((from, texts(&watchers().get("new-agent-test", "%5"))), (from, vec![]));
            watchers().stop(NEW_KEY);
        }
    });
}
