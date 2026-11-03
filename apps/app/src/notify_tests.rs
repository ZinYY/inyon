use std::cell::{Cell, RefCell};
use std::rc::Rc;

use inyon_protocol::{HostState, NotifyKind, ServerMsg};
use serde_json::{json, Value};

use super::*;

#[derive(Default)]
struct Tray {
    posted: RefCell<Vec<Notice>>,
    cleared: RefCell<Vec<String>>,
}

impl Shade for Tray {
    fn post(&self, n: &Notice) -> bool {
        self.posted.borrow_mut().push(n.clone());
        true
    }
    fn clear(&self, tag: &str) {
        self.cleared.borrow_mut().push(tag.to_owned());
    }
}

fn host(id: &str, status: &str, agent: Value) -> HostState {
    let pane = json!({ "id": "%3", "index": 0, "active": true, "command": "claude", "cwd": "/", "title": "", "width": 80, "height": 40, "dead": false, "agent": agent });
    serde_json::from_value(json!({ "config": { "id": id, "name": id, "kind": "link" }, "status": status, "integration": "installed", "tmux": "ok", "agents": ["claude"],
        "sessions": [{ "id": "$1", "name": "s", "attached": 0, "windows": [{ "id": "@1", "index": 1, "name": "", "active": true, "activity": 0, "panes": [pane] }] }], "updatedAt": 0 }))
    .unwrap()
}

fn asking(options: Value) -> Value {
    json!({ "kind": "claude", "pid": 1, "state": "waiting", "options": options })
}

fn notify(kind: NotifyKind) -> ServerMsg {
    ServerMsg::Notify { host: "lap top".into(), pane: "%3".into(), kind, title: "Claude".into(), body: "Run the tests?".into(), agent: None }
}

const ALL: Wants = Wants { done: true, pages: true };

fn setup() -> (Rc<Tray>, Rc<Notifier>) {
    let tray = Rc::new(Tray::default());
    (tray.clone(), Notifier::with_clock(tray, || 0.0))
}

#[test]
fn tags_and_paths_match_the_web_app() {
    assert_eq!(tag_of("box", Some("%3")), "box:%3");
    assert_eq!(tag_of("box", None), "machine:box");
    assert_eq!(href_of("lap top", Some("%12")), "/m/lap%20top/p/12");
    assert_eq!(href_of("hub:h1/box", None), "/machines/hub%3Ah1%2Fbox");
}

#[test]
fn settings_turn_finished_and_page_notifications_off_and_default_on() {
    assert_eq!(Wants::from_settings(None), ALL);
    assert_eq!(Wants::from_settings(Some("not json")), ALL);
    assert_eq!(Wants::from_settings(Some(r#"{"notifyDone":false,"theme":"dark"}"#)), Wants { done: false, pages: true });
    assert_eq!(Wants::from_settings(Some(r#"{"notifyPages":false}"#)), Wants { done: true, pages: false });
}

#[test]
fn nothing_is_posted_while_the_app_is_on_screen() {
    let (tray, n) = setup();
    assert!(n.heard(&notify(NotifyKind::Done), true, ALL).is_none());
    assert!(n.heard(&notify(NotifyKind::Waiting), true, ALL).is_none());
    assert!(tray.posted.borrow().is_empty());
}

#[test]
fn a_finished_turn_is_posted_at_once_unless_turned_off() {
    let (tray, n) = setup();
    assert!(n.heard(&notify(NotifyKind::Done), false, Wants { done: false, pages: true }).is_none());
    assert!(tray.posted.borrow().is_empty());
    assert!(n.heard(&notify(NotifyKind::Done), false, ALL).is_none());
    let p = tray.posted.borrow()[0].clone();
    assert_eq!((p.kind.as_str(), p.tag.as_str(), p.href.as_str()), ("done", "lap top:%3", "/m/lap%20top/p/3"));
    assert_eq!((p.host.as_deref(), p.pane.as_deref()), (Some("lap top"), Some("%3")));
}

#[test]
fn a_machine_going_offline_is_tagged_by_the_machine_and_opens_it() {
    let (tray, n) = setup();
    n.heard(&notify(NotifyKind::Offline), false, ALL);
    let p = tray.posted.borrow()[0].clone();
    assert_eq!((p.kind.as_str(), p.tag.as_str(), p.href.as_str()), ("offline", "machine:lap top", "/machines/lap%20top"));
    assert_eq!((p.host, p.pane), (None, None));
}

#[test]
fn a_question_waits_for_its_answers_and_offers_up_to_three() {
    let (tray, n) = setup();
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).expect("a question waits");
    assert!(tray.posted.borrow().is_empty());
    let hosts = vec![host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }, { "n": 2, "label": "No" }])))];
    n.ask(later, &hosts);
    let p = tray.posted.borrow()[0].clone();
    assert_eq!(p.kind, "waiting");
    assert_eq!(p.options, vec![Choice { n: 1, label: "Yes".into() }, Choice { n: 2, label: "No".into() }]);
    // four answers don't fit on a notification
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    let four = json!([{ "n": 1, "label": "a" }, { "n": 2, "label": "b" }, { "n": 3, "label": "c" }, { "n": 4, "label": "d" }]);
    n.ask(later, &[host("lap top", "online", asking(four))]);
    assert!(tray.posted.borrow()[1].options.is_empty());
}

#[test]
fn a_question_answered_before_its_answers_arrive_gets_no_notification() {
    let (tray, n) = setup();
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    // the agent finished before the overview came: the finished notice replaces the question
    n.heard(&notify(NotifyKind::Done), false, ALL);
    n.ask(later, &[host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))]);
    let kinds: Vec<_> = tray.posted.borrow().iter().map(|p| p.kind.clone()).collect();
    assert_eq!(kinds, ["done"]);
}

#[test]
fn a_finished_turn_ends_its_question_even_when_finished_turns_are_muted() {
    let (tray, n) = setup();
    let muted = Wants { done: false, pages: true };
    // before the answers arrive: no question is posted
    let later = n.heard(&notify(NotifyKind::Waiting), false, muted).unwrap();
    n.heard(&notify(NotifyKind::Done), false, muted);
    n.ask(later, &[host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))]);
    assert!(tray.posted.borrow().is_empty());
    // after it was posted: it's taken away, and nothing replaces it
    let later = n.heard(&notify(NotifyKind::Waiting), false, muted).unwrap();
    n.ask(later, &[host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))]);
    assert_eq!(tray.posted.borrow().len(), 1);
    tray.cleared.borrow_mut().clear();
    n.heard(&notify(NotifyKind::Done), false, muted);
    assert_eq!(*tray.cleared.borrow(), vec!["lap top:%3"]);
    assert_eq!(tray.posted.borrow().len(), 1);
    // a finished turn while on screen takes it away too
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    n.ask(later, &[host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))]);
    n.heard(&notify(NotifyKind::Done), true, ALL);
    assert_eq!(tray.cleared.borrow().len(), 2);
    assert_eq!(tray.posted.borrow().len(), 2);
}

#[test]
fn a_question_asked_again_posts_only_the_newer_one() {
    let (tray, n) = setup();
    let first = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    let mut again = notify(NotifyKind::Waiting);
    if let ServerMsg::Notify { body, .. } = &mut again {
        *body = "Edit the file?".into();
    }
    let second = n.heard(&again, false, ALL).unwrap();
    let hosts = [host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))];
    // the first one's wait ends first, and is stale by then
    n.ask(first, &hosts);
    assert!(tray.posted.borrow().is_empty());
    n.ask(second, &hosts);
    let bodies: Vec<_> = tray.posted.borrow().iter().map(|p| p.body.clone()).collect();
    assert_eq!(bodies, ["Edit the file?"]);
}

#[test]
fn a_question_answered_on_the_computer_takes_its_notification_away() {
    let (tray, n) = setup();
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    n.ask(later, &[host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }])))]);
    // still asking, or the machine went quiet: it stays
    n.overview(&[host("lap top", "online", asking(json!([])))]);
    n.overview(&[host("lap top", "connecting", json!({ "kind": "claude", "pid": 1, "state": "working" }))]);
    assert!(tray.cleared.borrow().is_empty());
    n.overview(&[host("lap top", "online", json!({ "kind": "claude", "pid": 1, "state": "working" }))]);
    assert_eq!(*tray.cleared.borrow(), vec!["lap top:%3"]);
    // and only once
    n.overview(&[host("lap top", "online", json!({ "kind": "claude", "pid": 1, "state": "idle" }))]);
    assert_eq!(tray.cleared.borrow().len(), 1);
}

#[test]
fn a_page_moving_on_is_posted_unless_turned_off() {
    let (tray, n) = setup();
    let page = ServerMsg::PageNotify { host: "box".into(), slug: "plan".into(), title: "Plan".into(), body: "8 of 12 done".into() };
    n.heard(&page, false, Wants { done: true, pages: false });
    assert!(tray.posted.borrow().is_empty());
    n.heard(&page, false, ALL);
    let p = tray.posted.borrow()[0].clone();
    assert_eq!((p.kind.as_str(), p.tag.as_str(), p.href.as_str()), ("page", "page:box/plan", "/pages/box/plan"));
}

#[test]
fn a_notice_reaches_kotlin_as_the_json_notifications_post_reads() {
    let n = Notice { title: "t".into(), body: "b".into(), href: "/".into(), tag: "x".into(), kind: "done".into(), host: None, pane: None, options: vec![] };
    assert_eq!(serde_json::to_string(&n).unwrap(), r#"{"title":"t","body":"b","href":"/","tag":"x","kind":"done"}"#);
}

fn tap(option: u32, label: &str, at: f64) -> Tap {
    Tap { host: "lap top".into(), pane: "%3".into(), option, label: label.into(), tag: "lap top:%3".into(), at }
}

fn yes_no() -> Vec<HostState> {
    vec![host("lap top", "online", asking(json!([{ "n": 1, "label": "Yes" }, { "n": 2, "label": "No" }])))]
}

/// Runs an answer whose sending finishes at once, and says what was sent.
fn answered(n: &Notifier, t: Tap, now: f64, hosts: &[HostState], result: Result<(), String>) -> Vec<(String, String, u32)> {
    let sent = RefCell::new(vec![]);
    let send = |h: String, p: String, o: u32| {
        sent.borrow_mut().push((h, p, o));
        std::future::ready(result)
    };
    futures_util::FutureExt::now_or_never(n.answer(t, now, hosts, send)).expect("finishes at once");
    sent.into_inner()
}

#[test]
fn a_tap_is_sent_only_while_the_session_asks_that_question() {
    let hosts = yes_no();
    assert_eq!(judge(&tap(1, "Yes", 0.0), 1_000.0, &hosts), Verdict::Send);
    // a different label at that number, a number it doesn't offer, another pane
    assert_eq!(judge(&tap(1, "No", 0.0), 1_000.0, &hosts), Verdict::Changed);
    assert_eq!(judge(&tap(3, "Yes", 0.0), 1_000.0, &hosts), Verdict::Changed);
    assert_eq!(judge(&Tap { pane: "%4".into(), ..tap(1, "Yes", 0.0) }, 1_000.0, &hosts), Verdict::Changed);
    // the agent moved on
    let working = vec![host("lap top", "online", json!({ "kind": "claude", "pid": 1, "state": "working" }))];
    assert_eq!(judge(&tap(1, "Yes", 0.0), 1_000.0, &working), Verdict::Changed);
}

#[test]
fn a_tap_older_than_twelve_seconds_is_dropped_without_a_word() {
    let (tray, n) = setup();
    assert_eq!(judge(&tap(1, "Yes", 0.0), ANSWER_TTL_MS, &yes_no()), Verdict::Send);
    assert_eq!(judge(&tap(1, "Yes", 0.0), ANSWER_TTL_MS + 1.0, &yes_no()), Verdict::Late);
    let sent = answered(&n, tap(1, "Yes", 0.0), 20_000.0, &yes_no(), Ok(()));
    assert!(sent.is_empty());
    // Kotlin's AnswerService has already said it wasn't sent
    assert!(tray.posted.borrow().is_empty() && tray.cleared.borrow().is_empty());
}

#[test]
fn a_sent_answer_takes_its_notification_away() {
    let (tray, n) = setup();
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    n.ask(later, &yes_no());
    let sent = answered(&n, tap(2, "No", 1_000.0), 2_000.0, &yes_no(), Ok(()));
    assert_eq!(sent, vec![("lap top".to_owned(), "%3".to_owned(), 2)]);
    assert_eq!(*tray.cleared.borrow(), vec!["lap top:%3"]);
    // no longer asking: the overview after it doesn't clear it again
    n.overview(&[host("lap top", "online", json!({ "kind": "claude", "pid": 1, "state": "working" }))]);
    assert_eq!(tray.cleared.borrow().len(), 1);
}

#[test]
fn a_changed_question_says_the_answer_was_not_sent() {
    let (tray, n) = setup();
    let sent = answered(&n, tap(1, "Allow", 1_000.0), 2_000.0, &yes_no(), Ok(()));
    assert!(sent.is_empty());
    let p = tray.posted.borrow()[0].clone();
    assert_eq!((p.title.as_str(), p.kind.as_str(), p.tag.as_str(), p.href.as_str()), ("That question changed", "done", "lap top:%3", "/m/lap%20top/p/3"));
    assert!(p.body.contains("“Allow” wasn't sent"), "{}", p.body);
    // no buttons: it doesn't offer the answers again
    assert!(p.options.is_empty());
}

#[test]
fn a_failed_answer_says_why_and_keeps_asking() {
    let (tray, n) = setup();
    let sent = answered(&n, tap(1, "Yes", 1_000.0), 2_000.0, &yes_no(), Err("lap top is offline".into()));
    assert_eq!(sent.len(), 1);
    let p = tray.posted.borrow()[0].clone();
    assert_eq!((p.title.as_str(), p.body.as_str(), p.kind.as_str()), ("Couldn't send your answer", "lap top is offline", "waiting"));
    assert!(tray.cleared.borrow().is_empty());
}

#[test]
fn a_tap_reaches_rust_as_kotlin_writes_it() {
    let t: Tap = serde_json::from_str(r#"{"host":"box","pane":"%7","option":2,"label":"No","tag":"box:%7","at":1730000000000}"#).unwrap();
    assert_eq!(t, Tap { host: "box".into(), pane: "%7".into(), option: 2, label: "No".into(), tag: "box:%7".into(), at: 1.73e12 });
}

/// A notifier whose clock the test sets.
fn timed() -> (Rc<Tray>, Rc<Notifier>, Rc<Cell<f64>>) {
    let (tray, clock) = (Rc::new(Tray::default()), Rc::new(Cell::new(0.0)));
    let c = clock.clone();
    (tray.clone(), Notifier::with_clock(tray, move || c.get()), clock)
}

/// The session asks a question, and its notification goes up at `at`.
fn asked(n: &Notifier, clock: &Cell<f64>, at: f64) {
    clock.set(at);
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    n.ask(later, &yes_no());
}

#[test]
fn a_tap_on_a_notification_since_replaced_by_a_newer_question_is_not_sent() {
    let (tray, n, clock) = timed();
    asked(&n, &clock, 1_000.0);
    // the same answers at the same numbers, asked again at 5 s; the tap was at 4 s, on the first
    asked(&n, &clock, 5_000.0);
    let sent = answered(&n, tap(1, "Yes", 4_000.0), 6_000.0, &yes_no(), Ok(()));
    assert!(sent.is_empty());
    // the newer question's notification stays as it is
    assert_eq!(tray.posted.borrow().len(), 2);
    assert!(tray.cleared.borrow().is_empty());
    // a tap on the newer one goes
    let sent = answered(&n, tap(1, "Yes", 5_500.0), 6_000.0, &yes_no(), Ok(()));
    assert_eq!(sent.len(), 1);
    assert_eq!(*tray.cleared.borrow(), vec!["lap top:%3"]);
}

#[test]
fn a_tap_while_a_newer_question_waits_for_its_answers_leaves_it_alone() {
    let (tray, n, clock) = timed();
    asked(&n, &clock, 1_000.0);
    // asked again; its notification goes up once the overview has its answers
    let later = n.heard(&notify(NotifyKind::Waiting), false, ALL).unwrap();
    // the tap no longer matches what's asked: no "That question changed" over the new one
    let sent = answered(&n, tap(1, "Allow", 2_000.0), 2_100.0, &yes_no(), Ok(()));
    assert!(sent.is_empty());
    assert_eq!(tray.posted.borrow().len(), 1);
    n.ask(later, &yes_no());
    assert_eq!(tray.posted.borrow().len(), 2);
    assert_eq!(tray.posted.borrow()[1].kind, "waiting");
}

#[test]
fn an_answer_that_lands_after_a_newer_question_keeps_that_notification() {
    let (tray, n, clock) = timed();
    asked(&n, &clock, 1_000.0);
    let n2 = n.clone();
    let c2 = clock.clone();
    // while the answer goes, the agent asks something new
    let send = move |_: String, _: String, _: u32| {
        asked(&n2, &c2, 3_000.0);
        std::future::ready(Ok(()))
    };
    futures_util::FutureExt::now_or_never(n.answer(tap(1, "Yes", 2_000.0), 2_000.0, &yes_no(), send)).unwrap();
    assert!(tray.cleared.borrow().is_empty(), "the new question's notification was cleared");
    // and the overview can still take it away once that one is answered
    n.overview(&[host("lap top", "online", json!({ "kind": "claude", "pid": 1, "state": "working" }))]);
    assert_eq!(*tray.cleared.borrow(), vec!["lap top:%3"]);
}

#[test]
fn a_send_stops_before_kotlin_says_it_was_not_sent() {
    assert_eq!(send_window_ms(1_000.0, 1_000.0), 14_000);
    assert_eq!(send_window_ms(1_000.0, 13_000.0), 2_000);
    assert_eq!(send_window_ms(1_000.0, 20_000.0), 0);
    // Kotlin's AnswerService.TTL_MS
    const { assert!(ANSWER_SEND_BY_MS < 15_000.0 && ANSWER_SEND_BY_MS > ANSWER_TTL_MS) };
}
