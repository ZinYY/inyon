//! claude/delivery.test.ts, test for test. Each test's name in the TS file is its doc comment.

use super::*;
use crate::claude::drafts::draft_rows;
use crate::claude::screen::{new_state, parse_screen, prompt};
use crate::test_support::screen_fixture;
use inyon_protocol::AgentKind;

fn state(working: bool, model: Option<&str>) -> ScreenState {
    let mut s = new_state(100);
    s.working = working;
    s.model = model.map(str::to_owned);
    s
}

/// missing composer and approval screens do not confirm delivery
#[test]
fn missing_composer_and_approval_screens_do_not_confirm_delivery() {
    assert!(!codex_accepted(&state(false, None), "hello"));
    let mut asking = state(false, Some("GPT-6-Luna"));
    asking.prompt = Some(prompt(vec![], vec![]));
    assert!(!codex_accepted(&asking, "hello"));
    let mut drafted = state(false, Some("GPT-6-Luna"));
    drafted.draft = Some("hello".into());
    assert!(!codex_accepted(&drafted, "hello"));
    assert!(codex_accepted(&state(true, Some("GPT-6-Luna")), "hello"));
}

/// unrelated or partial input is not enough to submit
#[test]
fn unrelated_or_partial_input_is_not_enough_to_submit() {
    assert!(!input_contains(Some("unrelated text of similar length"), "send this message", false));
    assert!(!input_contains(Some("this message"), "send this message", false));
    assert!(input_contains(Some("send\nthis message"), "send this message", false));
    // a paste preview hides its text: it's this message only when known to be (just typed, or mirrored)
    assert!(input_contains(Some("[Pasted text #1 +20 lines]"), "long message", true));
    assert!(!input_contains(Some("[Pasted text #1 +20 lines]"), "long message", false));
    // leftovers of an older draft in front of the paste would be sent along with it
    assert!(!input_contains(Some("Hey also for mobile I want [Pasted text #1 +20 lines]"), "long message", false));
}

/// a long dictated line needs a clear for every row it wraps onto
#[test]
fn a_long_dictated_line_needs_a_clear_for_every_row_it_wraps_onto() {
    assert_eq!(draft_rows("short", Some(100)), 3);
    assert_eq!(draft_rows(&"x".repeat(640), Some(100)), 640usize.div_ceil(94) + 2);
    assert_eq!(draft_rows(&format!("one\ntwo\n{}", "y".repeat(300)), Some(80)), 1 + 1 + 300usize.div_ceil(74) + 2);
}

/// real Codex queue confirms only the queued message, despite another draft
#[test]
fn real_codex_queue_confirms_only_the_queued_message_despite_another_draft() {
    let screen = parse_screen(&screen_fixture("codex", "queued"), Some(100), AgentKind::Codex);
    assert!(screen.working);
    assert!(codex_accepted(&screen, "second message queued"));
    assert!(!codex_accepted(&screen, "different message"));
}

/// Antigravity identifies each permission mode, including returning to default
#[test]
fn antigravity_identifies_each_permission_mode_including_returning_to_default() {
    let capture = |name: &str| parse_screen(&screen_fixture("antigravity", name), Some(150), AgentKind::Antigravity);
    assert_eq!(capture("accept-edits").mode.as_deref(), Some("accept-edits"));
    assert_eq!(capture("plan").mode.as_deref(), Some("plan"));
    assert_eq!(capture("idle").mode.as_deref(), Some("default"));
    assert_eq!(parse_screen(&[], Some(150), AgentKind::Antigravity).mode, None);
}

/// Codex image path conversion is recognized before submit and in its queue
#[test]
fn codex_image_path_conversion_is_recognized_before_submit_and_in_its_queue() {
    let text = "Check this\n\nAttached files:\n/tmp/a.png\n/tmp/b.jpg\n/tmp/notes.txt";
    let rendered = "Check this\n\nAttached files:\n[Image #1]\n[Image #2]\n/tmp/notes.txt";
    assert!(input_contains(Some(rendered), text, false));
    assert!(!input_contains(Some(&rendered.replacen("Check this", "Different text", 1)), text, false));
    assert!(!input_contains(Some(&rendered.replacen("[Image #2]", "[Image #3]", 1)), text, false));
    assert!(!input_contains(Some(&rendered.replacen("/tmp/notes.txt", "/tmp/other.txt", 1)), text, false));
    let mut s = state(true, Some("GPT-6-Luna"));
    s.draft = Some("new draft".into());
    s.queue = Some(vec![rendered.into()]);
    assert!(codex_accepted(&s, text));
}

/// Codex collapsed content preserves the real draft and checks its size
#[test]
fn codex_collapsed_content_preserves_the_real_draft_and_checks_its_size() {
    let text = "x".repeat(1824);
    assert!(has_paste_preview("[Pasted Content 1824 chars]"));
    assert!(input_contains(Some("[Pasted Content 1824 chars]"), &text, true));
    assert!(!input_contains(Some("[Pasted Content 1823 chars]"), &text, true));
    assert!(!input_contains(Some("[Pasted Content 1824 chars] extra"), &text, true));
    // an older paste of the same length isn't this message unless it was just put there
    assert!(!input_contains(Some("[Pasted Content 1824 chars]"), &text, false));
    assert!(has_paste_preview("[Pasted text #1 +20 lines]"));
}

/// an agent's record matches the sent message despite rewrapping, not a shorter draft
#[test]
fn an_agents_record_matches_the_sent_message_despite_rewrapping_not_a_shorter_draft() {
    let sent = "why can't I paste images? it says \"not supported\" really\n\nAttached files:\n/home/u/.inyon/uploads/a.png";
    assert!(same_message(&format!("<pasted_content id=\"x\">{}</pasted_content>", sent.replace('\n', " ")), sent));
    assert!(same_message(&sent.replace('"', "“"), sent));
    // a draft mirrored before the last edits is a prefix, and must not count as the message
    assert!(!same_message("why can't I paste images? it says \"not supported\" real", sent));
    // a very long message the transcript cut short still counts
    let long = "word ".repeat(1000);
    assert!(same_message(&long[..2500], &long));
}

/// a message with no words is matched only by the same text
#[test]
fn a_message_with_no_words_is_matched_only_by_the_same_text() {
    assert!(same_message("?", "?"));
    assert!(same_message(" ...\n", "..."));
    // the stand-in for items that aren't messages, and other wordless text, never match it
    assert!(!same_message("\0", "?"));
    assert!(!same_message("...", "?"));
    assert!(!same_message("Sure, here it is", "?"));
    assert!(!same_message("", "?"));
}

fn user(uuid: &str, text: &str, queued: Option<bool>, shell: Option<bool>) -> ChatItem {
    ChatItem::User { seq: 0, uuid: uuid.into(), ts: "2026-10-11T03:00:00Z".into(), text: text.into(), images: 0, queued, shell }
}

fn state_of(l: &SendLedger, id: &str) -> Option<SendStatus> {
    l.get(id).map(|s| s.state)
}

/// the ledger matches a send to the first new record of it, never an older look-alike
#[test]
fn the_ledger_matches_a_send_to_the_first_new_record_of_it_never_an_older_look_alike() {
    let mut ledger = SendLedger::new();
    let mut items = vec![user("a", "yes", None, None)];
    ledger.track("s1", "yes", &items, false);
    // the old "yes" was there before the send
    assert!(!ledger.match_items(&items, &[]));
    assert_eq!(state_of(&ledger, "s1"), Some(SendStatus::Sending));
    // queued while the agent works, then taken into the conversation
    assert!(ledger.match_items(&items, &["yes".into()]));
    assert_eq!(state_of(&ledger, "s1"), Some(SendStatus::Queued));
    items.push(user("b", "yes", Some(true), None));
    ledger.match_items(&items, &[]);
    let s = ledger.get("s1").unwrap();
    assert_eq!((s.state, s.uuid.as_deref()), (SendStatus::Delivered, Some("b")));
}

/// two identical sends claim two records, and a late record clears a failure
#[test]
fn two_identical_sends_claim_two_records_and_a_late_record_clears_a_failure() {
    let mut ledger = SendLedger::new();
    let mut items = vec![];
    ledger.track("s1", "continue", &items, false);
    ledger.track("s2", "continue", &items, false);
    items.push(user("x", "continue", None, None));
    ledger.match_items(&items, &[]);
    let s1 = ledger.get("s1").unwrap();
    assert_eq!((s1.state, s1.uuid.as_deref()), (SendStatus::Delivered, Some("x")));
    assert_eq!(state_of(&ledger, "s2"), Some(SendStatus::Sending));
    ledger.settle("s2", Some(SendStatus::Failed), Some("timed out".into()));
    items.push(user("y", "continue", None, None));
    ledger.match_items(&items, &[]);
    let s2 = ledger.get("s2").unwrap();
    assert_eq!((s2.state, s2.uuid.as_deref(), s2.error), (SendStatus::Delivered, Some("y"), None));
    // and a timeout can't undo a delivery
    ledger.settle("s1", Some(SendStatus::Sent), None);
    assert_eq!(state_of(&ledger, "s1"), Some(SendStatus::Delivered));
}

/// the ledger reads shell, slash and Codex image records as they were typed
#[test]
fn the_ledger_reads_shell_slash_and_codex_image_records_as_they_were_typed() {
    let mut ledger = SendLedger::new();
    let mut items = vec![];
    ledger.track("sh", "! ls -la", &items, false);
    ledger.track("cmd", "/compact keep the tests", &items, false);
    ledger.track("img", "look at this\n\nAttached files:\n/tmp/t/a.png", &items, false);
    items.push(user("1", "ls -la", None, Some(true)));
    items.push(ChatItem::Command { seq: 1, uuid: "2".into(), ts: String::new(), name: "compact".into(), args: "keep the tests".into(), output: None });
    items.push(user("3", "look at this\n\nAttached files:\n[Image #1]<image name=[Image #1] path=\"/tmp/t/a.png\">x</image>", None, None));
    ledger.match_items(&items, &[]);
    let got: Vec<_> = ledger.list().into_iter().map(|s| (s.id, s.state, s.uuid)).collect();
    let want = [("sh", "1"), ("cmd", "2"), ("img", "3")].map(|(id, u)| (id.to_owned(), SendStatus::Delivered, Some(u.to_owned())));
    assert_eq!(got, want);
}

/// a brand-new transcript (first message of a conversation) still matches
#[test]
fn a_brand_new_transcript_first_message_of_a_conversation_still_matches() {
    let mut ledger = SendLedger::new();
    ledger.track("s1", "hello there", &[user("old", "hello there", None, None)], false);
    // the watcher switched files: none of the new items were known
    ledger.match_items(&[user("new", "hello there", None, None)], &[]);
    let s = ledger.get("s1").unwrap();
    assert_eq!((s.state, s.uuid.as_deref()), (SendStatus::Delivered, Some("new")));
}

/// a setting from the app's controls is followed like any send but marked, so no device shows it as a message
#[test]
fn a_setting_from_the_apps_controls_is_followed_like_any_send_but_marked() {
    let mut ledger = SendLedger::new();
    ledger.track("m", "/model gpt-6.1-sol", &[], true);
    ledger.track("t", "hello", &[], false);
    let got: Vec<_> = ledger.list().into_iter().map(|s| (s.id, s.setting.unwrap_or(false))).collect();
    assert_eq!(got, vec![("m".to_owned(), true), ("t".to_owned(), false)]);
}

/// before typing, a draft that differs only in spaces or line breaks is not the message
#[test]
fn before_typing_a_draft_that_differs_only_in_spaces_or_line_breaks_is_not_the_message() {
    assert!(!input_contains(Some("print(\"A B\")"), "print(\"AB\")", false));
    assert!(input_contains(Some("def f():\n    pass"), "def f(): pass", false));
    // once typed, the terminal's wrapping may split a long word; that still counts
    assert!(input_contains(Some("https://example.com/a\n  very/long/path"), "https://example.com/avery/long/path", true));
}
