//! The action tests: claude/send.test.ts, answer.test.ts, empty-send.test.ts, ultracode.test.ts,
//! fork.test.ts, the session id checks of features/paths.test.ts, resumeCommand from
//! integration.test.ts and the psmux commands of hosts/win-prelude.test.ts. The TS spies on the
//! registry; here the pane is put in this thread's registry, with polls held as the stubbed
//! `registry.refresh` did, and the machine is a fake that answers like the agent would.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use inyon_protocol::{prompt_key, AgentKind, IntegrationState};
use regex::Regex;
use serde_json::json;

use super::*;
use crate::hosts::host::ExecResult;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::rt::run;
use crate::test_support::{machine, sh, TempDir};
use crate::tmux::tmux::{AgentProc, ProbeResult, TmuxProbe};

const R100: usize = 100;

fn rule(n: usize) -> String {
    "─".repeat(n)
}

/// send-keys' own quoting taken off
fn unq(s: &str) -> String {
    let s = s.strip_prefix('\'').unwrap_or(s);
    let s = s.strip_suffix('\'').unwrap_or(s);
    s.replace(r"'\''", "'")
}

fn keys_re() -> Regex {
    Regex::new(r"send-keys -t \S+ (?:-l -- (.*)|(\S+))$").unwrap()
}

/// A pane in this thread's registry that no poll reads again.
fn pane(host: &str, panes: serde_json::Value) -> crate::hosts::registry::Rt {
    let rt = machine(host, "local", panes);
    rt.borrow_mut().hold_polls();
    rt
}

/// What the last probe found in a pane (`registry.agentProc`).
fn with_proc(rt: &crate::hosts::registry::Rt, pane: &str, pid: f64, cwd: &str, args: &str) {
    let mut procs = IndexMap::new();
    procs.insert(pane.to_owned(), AgentProc { kind: AgentKind::Claude, pid, started_at: 0.0, cwd: Some(cwd.into()), resume: None, args: args.into(), pane_pid: 0.0, custom: None });
    let probe = ProbeResult {
        hostname: "box".into(),
        home: "/home/u".into(),
        os: "Linux".into(),
        now: 0.0,
        latency_ms: 0.0,
        tmux: TmuxProbe::Ok,
        integration: IntegrationState::Missing,
        sessions: vec![],
        agent_procs: procs,
        hook_panes: IndexMap::new(),
        status_panes: IndexMap::new(),
        account: None,
        compact_panes: IndexMap::new(),
        installed: vec![],
        signed_in: vec![],
        agy_conversations: IndexMap::new(),
        agy_status: IndexMap::new(),
        agy_titles: IndexMap::new(),
        agy_integration: None,
        hooked: vec![],
        machine_kind: None,
        keys_at: Default::default(),
        unwatched: Default::default(),
        claude_parked: Default::default(),
    };
    rt.borrow_mut().probe = Some(Rc::new(RefCell::new(probe)));
}

// ---------- send.test.ts ----------

#[derive(Clone)]
enum Piece {
    Text(String),
    Paste(String),
}

#[derive(Default)]
struct ClaudeBox {
    pieces: Vec<Piece>,
    pastes: usize,
    sent: Vec<String>,
    /// the box hasn't drawn what was typed yet
    slow: bool,
    /// a background shell's details panel covers the box and takes every key; Esc closes it
    panel: bool,
    escapes: usize,
}

/// Claude's input box as far as a send can tell: a long paste shows as "[Pasted text #N]" and Enter
/// submits what's behind it, so a preview left over from an older draft looks like any other.
fn fake_claude(start: Vec<Piece>) -> Rc<RefCell<ClaudeBox>> {
    let pastes = start.iter().filter(|p| matches!(p, Piece::Paste(_))).count();
    let b = Rc::new(RefCell::new(ClaudeBox { pieces: start, pastes, sent: vec![], slow: false, panel: false, escapes: 0 }));
    let state = b.clone();
    let re = keys_re();
    Fake::install(move |_, cmd, input| {
        let mut b = state.borrow_mut();
        if cmd.contains("@@CAP") && b.panel {
            let r = rule(R100);
            return ok(&format!("@@CAP %9\n● Done.\n\n✻ Worked for 10s · 2 shells still running\n{r}\n  Shell details\n\n  Status:   running\n  Command:  node serve.mjs\n\n  Output:\n  │ Serving . at http://127.0.0.1:8790/\n"));
        }
        if b.panel {
            if cmd.split(" && ").filter_map(|part| re.captures(part)).any(|k| k.get(2).is_some_and(|m| m.as_str() == "Escape")) {
                b.panel = false;
                b.escapes += 1;
            }
            return ok("");
        }
        if cmd.contains("@@CAP") {
            let pastes = b.pastes;
            let shown: String = if b.slow {
                String::new()
            } else {
                b.pieces.iter().enumerate().map(|(i, p)| if let Piece::Text(t) = p { t.clone() } else { format!("[Pasted text #{}]", i + pastes) }).collect()
            };
            let r = rule(R100);
            return ok(&format!("@@CAP %9\n● Done.\n{r}\n❯ {shown}\n{r}\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n"));
        }
        if cmd.contains("paste-buffer") {
            b.pastes += 1;
            let input = input.unwrap_or("").to_owned();
            b.pieces.push(if input.encode_utf16().count() > 800 { Piece::Paste(input) } else { Piece::Text(input) });
        } else {
            for part in cmd.split(" && ") {
                let Some(k) = re.captures(part) else { continue };
                if let Some(t) = k.get(1) {
                    b.pieces.push(Piece::Text(unq(t.as_str())));
                } else {
                    match &k[2] {
                        "C-u" => b.pieces.clear(),
                        "BSpace" => {
                            b.pieces.pop();
                        }
                        "Enter" => {
                            let all: String = b
                                .pieces
                                .iter()
                                .map(|p| match p {
                                    Piece::Text(t) | Piece::Paste(t) => t.clone(),
                                })
                                .collect();
                            b.sent.push(all);
                            b.pieces.clear();
                        }
                        _ => {}
                    }
                }
            }
        }
        ok("")
    });
    pane("local", json!([{ "id": "%9", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
    b
}

/// A background shell's details panel, opened by accident from Claude's "↓ to manage", took every
/// key of the next message: the send failed and the message never arrived (a laptop, 26 Oct 2026).
#[test]
fn a_shell_details_panel_over_the_box_is_closed_with_esc_and_the_message_goes_in() {
    run(|| async {
        let b = fake_claude(vec![]);
        b.borrow_mut().panel = true;
        send_message_to_pane("local", "%9", "start doing next things", true, false).await.unwrap();
        assert_eq!((b.borrow().escapes, b.borrow().sent.clone()), (1, vec!["start doing next things".to_owned()]));
        assert!(!claude_details_panel(&["● Done.".into(), "  Shell details".into(), "─".repeat(20), "❯ ".into(), "─".repeat(20)]));
    });
}

#[test]
fn a_box_slow_to_show_the_text_gets_it_typed_once_never_twice() {
    run(|| async {
        let b = fake_claude(vec![]);
        b.borrow_mut().slow = true;
        let _ = send_message_to_pane("local", "%9", "Run the tests again", true, false).await;
        assert_eq!(b.borrow().sent, ["Run the tests again"]);
    });
}

#[test]
fn a_paste_preview_left_by_an_older_draft_is_replaced_not_sent() {
    run(|| async {
        let message = format!("Hey so I have one problem with how we load messages. {}", "and then some more words ".repeat(60));
        // the mirror pasted the draft while it was still being dictated: a shorter start of the message
        let b = fake_claude(vec![Piece::Paste(message[..918].to_owned())]);
        send_message_to_pane("local", "%9", &message, true, false).await.unwrap();
        assert_eq!(b.borrow().sent, [message]);
    });
}

#[test]
fn a_short_message_typed_into_an_empty_box_goes_in_as_typed() {
    run(|| async {
        let b = fake_claude(vec![]);
        send_message_to_pane("local", "%9", "hello there", true, false).await.unwrap();
        assert_eq!(b.borrow().sent, ["hello there"]);
    });
}

#[test]
fn an_old_draft_that_differs_only_in_whitespace_is_replaced_not_sent() {
    run(|| async {
        let b = fake_claude(vec![Piece::Text(r#"print("A B")"#.into())]);
        send_message_to_pane("local", "%9", r#"print("AB")"#, true, false).await.unwrap();
        assert_eq!(b.borrow().sent, [r#"print("AB")"#]);
    });
}

/// Ghostex #254: a long prompt was pasted but never sent. A message over the paste threshold goes
/// in as a paste, and Enter follows once its preview shows in the box.
#[test]
fn a_long_message_goes_in_as_a_paste_and_is_sent() {
    run(|| async {
        let b = fake_claude(vec![]);
        let message = "Look over the failing tests and fix them. ".repeat(40);
        send_message_to_pane("local", "%9", &message, true, false).await.unwrap();
        assert_eq!(b.borrow().sent, [message]);
    });
}

/// Gemini's box as a send sees it: no parser reads it, so the screen is all there is. `shows`
/// says whether typed text appears there.
fn fake_gemini(shows: bool) -> Rc<RefCell<(String, Vec<String>)>> {
    let b = Rc::new(RefCell::new((String::new(), Vec::new())));
    let state = b.clone();
    let re = keys_re();
    Fake::install(move |_, cmd, input| {
        let mut b = state.borrow_mut();
        if cmd.contains("@@CAP") {
            let shown = if shows { b.0.clone() } else { String::new() };
            return ok(&format!("@@CAP %4\n✦ Done.\n╭────────────╮\n│ > {shown} │\n╰────────────╯\n"));
        }
        if cmd.contains("paste-buffer") {
            b.0.push_str(input.unwrap_or(""));
        }
        for part in cmd.split(" && ") {
            if let Some(k) = re.captures(part) {
                match k.get(1) {
                    Some(t) => b.0.push_str(&unq(t.as_str())),
                    None if &k[2] == "Enter" => {
                        let sent = std::mem::take(&mut b.0);
                        b.1.push(sent);
                    }
                    None => {}
                }
            }
        }
        ok("")
    });
    pane("local", json!([{ "id": "%4", "width": 100, "agent": { "kind": "gemini", "pid": 7, "state": "idle" } }]));
    b
}

#[test]
fn an_agent_without_a_screen_parser_gets_enter_once_the_text_shows() {
    run(|| async {
        let b = fake_gemini(true);
        send_message_to_pane("local", "%4", "Summarise the failing tests", true, false).await.unwrap();
        assert_eq!(b.borrow().1, ["Summarise the failing tests"]);
    });
}

#[test]
fn an_agent_without_a_screen_parser_gets_no_blind_enter() {
    run(|| async {
        let b = fake_gemini(false);
        let e = send_message_to_pane("local", "%4", "Summarise the failing tests", true, false).await.unwrap_err();
        assert!(e.message.contains("Enter wasn't pressed"), "{}", e.message);
        assert!(b.borrow().1.is_empty());
    });
}

#[test]
fn held_messages_wait_for_the_agent_to_be_free_then_go_in_order() {
    run(|| async {
        crate::platform::store::use_store(Rc::new(crate::platform::store::MemoryStore::new()));
        let b = fake_gemini(true);
        let first = crate::claude::held::hold("local", "%4", "Write the migration").unwrap();
        let second = crate::claude::held::hold("local", "%4", "Then run the tests").unwrap();
        assert!(crate::claude::held::hold("local", "%4", "  ").is_err());
        assert_eq!(crate::claude::held::hold("local", "%9", "x").unwrap_err().status, Some(404));
        // reordered and edited while held
        crate::claude::held::move_to("local", "%4", &second.id, 0).unwrap();
        crate::claude::held::edit("local", "%4", &first.id, "Write the migration for users").unwrap();
        let ids: Vec<String> = crate::claude::held::list("local", "%4").into_iter().map(|p| p.text).collect();
        assert_eq!(ids, ["Then run the tests", "Write the migration for users"]);
        // kept in the record
        let saved = crate::platform::store::kv::get_value(crate::claude::held::KEY).unwrap().to_string();
        assert!(saved.contains("Then run the tests"), "{saved}");
        // idle, but not for long enough yet: nothing goes
        let t0 = 1_000_000;
        crate::claude::held::tick(t0).await;
        sleep(50).await;
        assert!(b.borrow().1.is_empty());
        crate::claude::held::tick(t0 + crate::claude::held::SETTLE_MS).await;
        for _ in 0..200 {
            if !b.borrow().1.is_empty() && crate::claude::held::list("local", "%4").len() == 1 {
                break;
            }
            sleep(100).await;
        }
        assert_eq!(b.borrow().1, ["Then run the tests"]);
        assert_eq!(crate::claude::held::list("local", "%4").len(), 1);
        // dropped
        crate::claude::held::remove("local", "%4", &first.id).unwrap();
        assert!(crate::claude::held::list("local", "%4").is_empty());
    });
}

#[test]
fn a_held_message_that_fails_stays_first_with_the_reason_and_holding_stops() {
    run(|| async {
        crate::platform::store::use_store(Rc::new(crate::platform::store::MemoryStore::new()));
        let b = fake_gemini(false);
        let p = crate::claude::held::hold("local", "%4", "Summarise the failing tests").unwrap();
        assert!(crate::claude::held::send_now("local", "%4", &p.id).await.is_err());
        let left = crate::claude::held::list("local", "%4");
        assert_eq!(left.len(), 1);
        assert!(left[0].error.as_deref().unwrap().contains("Enter wasn't pressed"));
        // the tick leaves it alone until it's edited, sent now or dropped
        crate::claude::held::tick(1_000_000).await;
        crate::claude::held::tick(1_000_000 + crate::claude::held::SETTLE_MS).await;
        sleep(200).await;
        assert!(b.borrow().1.is_empty());
        crate::claude::held::edit("local", "%4", &p.id, "Summarise the failing tests, briefly").unwrap();
        assert_eq!(crate::claude::held::list("local", "%4")[0].error, None);
    });
}

#[test]
fn two_stops_at_once_send_one_escape() {
    run(|| async {
        let escapes = Rc::new(RefCell::new(0));
        let seen = escapes.clone();
        let working = include_str!("../../../../fixtures/screens/claude/working.txt").to_owned();
        Fake::install(move |_, cmd, _| {
            if cmd.contains("@@CAP") {
                let screen = if *seen.borrow() == 0 { working.clone() } else { working.replace("✽ Undulating… (2s · thinking with high effort)", "● Stopped.") };
                return ok(&format!("@@CAP %9\n{screen}"));
            }
            if cmd.contains("send-keys") && cmd.contains("Escape") {
                *seen.borrow_mut() += 1;
            }
            ok("")
        });
        pane("local", json!([{ "id": "%9", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        // the phone and the laptop press Stop together, then once more straight after
        let (a, b) = futures_util::future::join(interrupt("local", "%9"), interrupt("local", "%9")).await;
        a.unwrap();
        b.unwrap();
        interrupt("local", "%9").await.unwrap();
        assert_eq!(*escapes.borrow(), 1);
    });
}

#[test]
fn a_new_session_with_an_unknown_agent_is_refused_before_anything_is_created() {
    run(|| async {
        let fake = Fake::install(|_, _, _| ok("%5\n"));
        pane("local", json!([]));
        let opts = LaunchOpts { agent: Some("nonsense".into()), ..Default::default() };
        assert_eq!(launch("local", &opts).await.unwrap_err().message, "Unknown agent");
        assert!(fake.cmds().is_empty());
    });
}

#[test]
fn a_custom_agent_starts_with_its_own_command_and_the_flags_of_the_agent_it_works_like() {
    run(|| async {
        use crate::agents::custom::{set, CustomAgent};
        let fake = Fake::install(|_, cmd, _| if cmd.contains("command -v") { ok("claude\n") } else { ok("%5\n") });
        pane("local", json!([]));
        set(vec![CustomAgent { id: "yolo".into(), name: "Claude YOLO".into(), command: "claude --dangerously-skip-permissions".into(), like: AgentKind::Claude }]);
        let opts = LaunchOpts { custom: Some("yolo".into()), agent: Some("codex".into()), model: Some("opus".into()), ..Default::default() };
        assert_eq!(launch("local", &opts).await.unwrap(), "%5");
        let cmds = fake.cmds();
        assert!(cmds.iter().any(|c| c.contains("n='Claude YOLO'")), "{cmds:?}");
        assert!(cmds.iter().any(|c| c.contains("claude --dangerously-skip-permissions --model opus")), "{cmds:?}");
        let gone = LaunchOpts { custom: Some("gone".into()), ..Default::default() };
        let before = fake.cmds().len();
        assert_eq!(launch("local", &gone).await.unwrap_err().message, "Unknown custom agent");
        assert_eq!(fake.cmds().len(), before);
        set(vec![]);
    });
}

#[test]
fn a_folder_or_name_holding_a_tmux_format_reaches_tmux_as_plain_text() {
    run(|| async {
        let fake = Fake::install(|_, _, _| ok("%5\n"));
        pane("local", json!([{ "id": "%3", "agent": null, "command": "zsh" }]));
        let opts = LaunchOpts { cwd: Some("/w/x#(touch /tmp/p)".into()), name: Some("a#(id)".into()), ..Default::default() };
        launch("local", &opts).await.unwrap();
        let cmd = fake.cmds().last().cloned().unwrap();
        // the folder test reads it as is; tmux gets every # doubled; # is out of session names
        assert!(cmd.contains("[ -d '/w/x#(touch /tmp/p)' ]"), "{cmd}");
        assert!(cmd.contains("-c '/w/x##(touch /tmp/p)'"), "{cmd}");
        assert!(cmd.contains("n='a-(id)'"), "{cmd}");
        rename_pane("local", "%3", "build #(id)").await.unwrap();
        assert!(fake.cmds().iter().any(|c| c == "tmux rename-window -t '%3' 'build ##(id)'"), "{:?}", fake.cmds());
        assert!(respawn_shell(false, "'%1'", Some("/srv/x#y")).contains("-c '/srv/x##y'"));
        // psmux is left as it was
        assert!(respawn_shell(true, "'%1'", Some("C:/code/C#")).contains("-c 'C:/code/C#'"));
    });
}

// ---------- answer.test.ts ----------

fn screen_for(body: &str) -> Vec<String> {
    [body, "", "Do you want to proceed?", "❯ 1. Yes", "  2. No", "", "Esc to cancel"].map(str::to_owned).to_vec()
}

#[test]
fn an_answer_meant_for_a_question_already_answered_elsewhere_never_lands_on_the_next_one() {
    run(|| async {
        let seen = screen_for("Allow reading README.md?");
        let now = screen_for("Allow replacing the deployment configuration?");
        let keys = Rc::new(RefCell::new(Vec::<String>::new()));
        let k = keys.clone();
        let shown = now.join("\n");
        let _fake = Fake::install(move |_, cmd, _| {
            if cmd.contains("capture-pane") {
                return ok(&format!("@@CAP %7\n{shown}"));
            }
            if cmd.contains("send-keys") {
                k.borrow_mut().push(cmd.to_owned());
            }
            ok("")
        });
        pane("box", json!([{ "id": "%7", "width": 120, "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
        let key = |lines: &[String]| {
            let p = parse_screen(lines, Some(120), AgentKind::Claude).prompt.unwrap();
            prompt_key(&p.body, &p.options)
        };

        // the card showed the README question; the screen now asks about the deployment config
        let err = answer("box", "%7", 1, None, Some(&key(&seen))).await.unwrap_err();
        assert!(err.message.contains("another one is waiting"), "{}", err.message);
        assert!(keys.borrow().is_empty());
        // answering the question that's actually there works
        answer("box", "%7", 1, None, Some(&key(&now))).await.unwrap();
        assert_eq!(keys.borrow().len(), 1);
        assert!(keys.borrow()[0].ends_with(" 1"), "{}", keys.borrow()[0]);
    });
}

/// Ghostex #251: answers to AskUserQuestion from the chat never reached the agent. A pick goes in
/// as its number, and a typed answer as the "Type something" number followed by the text.
#[test]
fn a_question_from_ask_user_question_takes_a_pick_and_a_typed_answer() {
    run(|| async {
        let shown = crate::test_support::screen_fixture("claude", "question-single").join("\n");
        let sent = Rc::new(RefCell::new(Vec::<(String, Option<String>)>::new()));
        let s = sent.clone();
        let _fake = Fake::install(move |_, cmd, input| {
            if cmd.contains("capture-pane") {
                return ok(&format!("@@CAP %7\n{shown}"));
            }
            if cmd.contains("send-keys") || cmd.contains("paste-buffer") {
                s.borrow_mut().push((cmd.to_owned(), input.map(str::to_owned)));
            }
            ok("")
        });
        pane("box", json!([{ "id": "%7", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        answer("box", "%7", 2, None, None).await.unwrap();
        assert_eq!(sent.borrow().len(), 1);
        assert!(sent.borrow()[0].0.ends_with(" 2"), "{:?}", sent.borrow());
        sent.borrow_mut().clear();
        answer("box", "%7", 4, Some("Teal"), None).await.unwrap();
        let sent = sent.borrow();
        assert!(sent[0].0.ends_with(" 4"), "{sent:?}");
        assert!(sent.iter().skip(1).any(|(cmd, input)| cmd.contains("Teal") || input.as_deref().is_some_and(|i| i.contains("Teal"))), "{sent:?}");
    });
}

/// With previews, a number only moved the highlight: the chat's pick was taken (200) and nothing
/// was answered. It moves the highlight and presses Enter.
#[test]
fn a_question_with_previews_is_answered_with_the_arrows_and_enter() {
    run(|| async {
        let shown = crate::test_support::screen_fixture("claude", "question-preview-tabs").join("\n");
        let sent = Rc::new(RefCell::new(Vec::<String>::new()));
        let s = sent.clone();
        let _fake = Fake::install(move |_, cmd, _| {
            if cmd.contains("capture-pane") {
                return ok(&format!("@@CAP %8\n{shown}"));
            }
            if cmd.contains("send-keys") {
                s.borrow_mut().push(cmd.to_owned());
            }
            ok("")
        });
        pane("preview", json!([{ "id": "%8", "width": 200, "agent": { "kind": "claude", "pid": 7, "state": "working" } }]));
        answer("preview", "%8", 2, None, None).await.unwrap();
        let sent = sent.borrow();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].contains("Down") && sent[0].trim_end().ends_with("Enter") && !sent[0].ends_with(" 2"), "{sent:?}");
    });
}

// ---------- empty-send.test.ts ----------

#[test]
fn an_empty_message_is_refused_before_it_counts_as_a_send() {
    run(|| async {
        let _fake = Fake::install(|_, _, _| ok(""));
        pane("empty-send", json!([{ "id": "%9", "title": "claude", "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
        let err = send_message("empty-send", "%9", "   ", true, Some("app-empty"), false, false).await.unwrap_err();
        assert_eq!((err.status, err.message.as_str()), (Some(400), "Message is empty"));
        // nothing is followed, so the next thing the agent writes isn't taken for it
        let w = watchers().get("empty-send", "%9");
        w.push_item(
            serde_json::from_value(json!({ "kind": "assistant", "uuid": "reply-1", "ts": "", "seq": 0, "msgId": "m1", "block": { "type": "text", "text": "Done." } })).unwrap(),
        );
        assert_eq!(w.delivery("app-empty"), None);
        watchers().stop("empty-send:%9");
    });
}

// ---------- ultracode.test.ts ----------

#[derive(Default)]
struct UltraPane {
    on: bool,
    text: String,
    said: String,
    sent: Vec<String>,
}

/// A Claude pane as far as the switch can tell: its box, the "ultracode" label on the box's rule
/// while the switch is on, and what Claude prints under `/effort ultracode` when it can't.
fn fake_ultra(on: bool, refuse: Option<&str>) -> Rc<RefCell<UltraPane>> {
    let p = Rc::new(RefCell::new(UltraPane { on, ..Default::default() }));
    let state = p.clone();
    let refuse = refuse.map(str::to_owned);
    let re = keys_re();
    let toggle = Regex::new(r"^/effort ultracode (on|off)$").unwrap();
    Fake::install(move |_, cmd, _| {
        let mut p = state.borrow_mut();
        if cmd.contains("@@CAP") {
            let r = rule(R100);
            let top = if p.on { format!("{} ultracode ─", rule(88)) } else { r.clone() };
            let answer = if p.said.is_empty() { String::new() } else { format!("❯ /effort ultracode on\n  ⎿  {}\n", p.said) };
            return ok(&format!("@@CAP %9\n● Done.\n{answer}{top}\n❯ {}\n{r}\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n", p.text));
        }
        for part in cmd.split(" && ") {
            let Some(k) = re.captures(part) else { continue };
            if let Some(t) = k.get(1) {
                let t = unq(t.as_str());
                p.text.push_str(&t);
            } else {
                match &k[2] {
                    "C-u" => p.text.clear(),
                    "BSpace" => {
                        p.text.pop();
                    }
                    "Enter" => {
                        let text = std::mem::take(&mut p.text);
                        if let Some(m) = toggle.captures(&text) {
                            match &refuse {
                                Some(r) => p.said = r.clone(),
                                None => p.on = &m[1] == "on",
                            }
                        }
                        p.sent.push(text);
                    }
                    _ => {}
                }
            }
        }
        ok("")
    });
    pane("local", json!([{ "id": "%9", "width": 100, "agent": { "kind": "claude", "pid": 7, "state": "idle" } }]));
    p
}

#[test]
fn turning_ultracode_on_types_the_command_and_the_label_on_the_box_confirms_it() {
    run(|| async {
        let p = fake_ultra(false, None);
        set_ultracode("local", "%9", true).await.unwrap();
        assert_eq!(p.borrow().sent, ["/effort ultracode on"]);
    });
}

#[test]
fn turning_ultracode_off_the_same_way() {
    run(|| async {
        let p = fake_ultra(true, None);
        set_ultracode("local", "%9", false).await.unwrap();
        assert_eq!(p.borrow().sent, ["/effort ultracode off"]);
    });
}

#[test]
fn ultracode_already_as_wanted_nothing_is_typed() {
    run(|| async {
        let p = fake_ultra(true, None);
        set_ultracode("local", "%9", true).await.unwrap();
        assert!(p.borrow().sent.is_empty());
    });
}

#[test]
fn when_claude_cant_its_own_answer_is_the_error() {
    run(|| async {
        fake_ultra(false, Some("Ultracode needs dynamic workflows enabled (see /config). Valid options are: on, off"));
        let err = set_ultracode("local", "%9", true).await.unwrap_err();
        assert!(err.message.contains("Ultracode needs dynamic workflows enabled (see /config)"), "{}", err.message);
    });
}

#[test]
fn ultracode_not_a_claude_pane_refused_before_anything_is_typed() {
    run(|| async {
        let fake = Fake::install(|_, _, _| ok(""));
        pane("local", json!([{ "id": "%9", "width": 100, "agent": { "kind": "codex", "pid": 7, "state": "idle" } }]));
        let err = set_ultracode("local", "%9", true).await.unwrap_err();
        assert!(err.message.contains("Claude Code"), "{}", err.message);
        assert!(fake.cmds().is_empty());
    });
}

// ---------- fork.test.ts ----------

const FORK_ID: &str = "0b6e2a52-3c1d-4f7a-9e44-5d1f0c2b7a19";

fn fork_host() -> Rc<Fake> {
    Fake::install(|_, cmd, _| -> ExecResult {
        if cmd.starts_with("test -s") {
            return ok("yes\n");
        }
        if cmd.contains("new-window") {
            return ok("%9\n");
        }
        if cmd.contains("capture-pane") {
            return ok("@@CAP %9\nuser@box ~/app $ \n");
        }
        if cmd.contains("/cmdline") {
            let bin = if cmd.contains("/41/") { "codex" } else { "claude" };
            return ok(&format!("/usr/bin/node\n/home/u/.local/bin/{bin}\n--model\nopus\n"));
        }
        ok("")
    })
}

fn fork_pane(kind: &str, session: Option<&str>) -> crate::hosts::registry::Rt {
    pane("h", json!([{ "id": "%1", "cwd": "/home/u/app", "agent": { "kind": kind, "pid": 7, "state": "idle", "sessionId": session, "transcriptPath": "/t.jsonl" } }]))
}

/// what was typed, with send-keys' own quoting taken off
fn typed(fake: &Fake) -> String {
    fake.cmds().iter().filter(|c| c.contains("send-keys")).map(|c| c.replace(r"'\''", "'")).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_claude_conversation_forks_into_a_new_window_same_folder_and_flags() {
    run(|| async {
        let fake = fork_host();
        let rt = fork_pane("claude", Some(FORK_ID));
        with_proc(&rt, "%1", 40.0, "/home/u/app", "claude --model opus");
        assert_eq!(fork_conversation("h", "%1").await.unwrap(), "%9");
        assert_eq!(fake.cmds().into_iter().find(|c| c.contains("new-window")).unwrap(), "tmux new-window -t 'work' -c '/home/u/app' -P -F '#{pane_id}'");
        let t = typed(&fake);
        assert!(t.contains(&format!("-t '%9' -l -- 'claude --resume {FORK_ID} '--model' 'opus' --fork-session'")), "{t}");
    });
}

#[test]
fn codex_uses_its_fork_command() {
    run(|| async {
        let fake = fork_host();
        let rt = fork_pane("codex", Some(FORK_ID));
        with_proc(&rt, "%1", 41.0, "/home/u/app", "codex --model opus");
        fork_conversation("h", "%1").await.unwrap();
        let t = typed(&fake);
        assert!(t.contains(&format!("codex fork {FORK_ID} '--model' 'opus'")), "{t}");
    });
}

#[test]
fn nothing_to_fork_before_the_first_message_and_not_every_agent_forks() {
    run(|| async {
        let _fake = fork_host();
        fork_pane("claude", None);
        assert!(fork_conversation("h", "%1").await.unwrap_err().message.contains("No conversation to fork yet"));
        fork_pane("antigravity", Some(FORK_ID));
        assert!(fork_conversation("h", "%1").await.unwrap_err().message.contains("Only Claude Code, Codex and OpenCode"));
    });
}

// ---------- features/paths.test.ts: session ids ----------

#[test]
fn a_resumed_session_id_must_be_a_uuid_before_it_is_typed_into_a_shell() {
    for good in [None, Some(json!(null)), Some(json!("")), Some(json!("7409eef9-d507-4808-a592-0bfabe896e3a"))] {
        assert!(is_session_id(good.as_ref(), None), "{good:?}");
    }
    for bad in [json!("7409eef9-d507-4808-a592-0bfabe896e3a; rm -rf ~"), json!("$(id)"), json!("latest"), json!(["7409eef9-d507-4808-a592-0bfabe896e3a"]), json!(42)] {
        assert!(!is_session_id(Some(&bad), None), "{bad}");
    }
}

#[test]
fn opencodes_own_session_ids_resume_it_only_opencodes_and_nothing_a_shell_reads_as_more() {
    assert!(is_session_id_str(Some("ses_0123456789abcdef"), Some(AgentKind::Opencode)));
    assert!(!is_session_id_str(Some("ses_0123456789abcdef"), Some(AgentKind::Claude)));
    for bad in ["ses_; id", "ses_$(id)", "ses_", "ses_a b"] {
        assert!(!is_session_id_str(Some(bad), Some(AgentKind::Opencode)), "{bad}");
    }
}

// ---------- integration.test.ts: resumeCommand ----------

#[test]
fn resume_keeps_launch_flags_drops_the_old_session_and_any_prompt() {
    let id = "7409eef9-d507-4808-a592-0bfabe896e3a";
    let s = Some(id);
    assert_eq!(resume_command(AgentKind::Claude, s, "claude --dangerously-skip-permissions"), format!("claude --resume {id} '--dangerously-skip-permissions'"));
    assert_eq!(
        resume_command(AgentKind::Claude, s, "/home/u/.local/bin/claude --resume 11111111-2222-3333-4444-555555555555 --model opus fix it"),
        format!("claude --resume {id} '--model' 'opus'")
    );
    assert_eq!(resume_command(AgentKind::Claude, s, "claude -c --permission-mode plan"), format!("claude --resume {id} '--permission-mode' 'plan'"));
    assert_eq!(resume_command(AgentKind::Codex, s, "node /usr/lib/node_modules/@openai/codex/bin/codex.js -m gpt-5 --yolo"), format!("codex resume {id} '-m' 'gpt-5' '--yolo'"));
    assert_eq!(
        resume_command(AgentKind::Antigravity, s, "/home/u/.local/bin/agy --conversation 11111111-2222-3333-4444-555555555555 --mode plan -p hi --dangerously-skip-permissions"),
        format!("agy --conversation {id} '--mode' 'plan' '--dangerously-skip-permissions'")
    );
    assert_eq!(resume_command(AgentKind::Antigravity, None, "agy -c --effort low"), "agy '--effort' 'low'");
}

/// From "a restart keeps a flag value with spaces in it whole, read from the real argument list":
/// where there is no /proc to read, there is no argument list
#[test]
fn a_machine_without_proc_gives_no_argument_list() {
    run(|| async {
        let fake = Fake::install(|_, _, _| ExecResult { code: 1, stdout: String::new(), stderr: String::new() });
        assert_eq!(argv(&crate::test_support::host("mac", "local"), 4242.0).await, None);
        assert_eq!(fake.cmds(), [r"tr '\0' '\n' < /proc/4242/cmdline 2>/dev/null"]);
    });
}

// ---------- end-session.test.ts: what closing a pane takes ----------

#[test]
fn what_closing_a_pane_takes_with_it() {
    let found = |windows: usize, panes: usize| -> FoundPane {
        let pane = json!({ "id": "%1", "index": 0, "active": true, "command": "sh", "cwd": "/", "title": "", "width": 80, "height": 24, "dead": false });
        let window = json!({ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": vec![pane.clone(); panes] });
        let session = json!({ "id": "$1", "name": "s", "attached": 0, "windows": vec![window.clone(); windows] });
        FoundPane { session: serde_json::from_value(session).unwrap(), window: serde_json::from_value(window).unwrap(), pane: serde_json::from_value(pane).unwrap(), home: None }
    };
    assert_eq!(closes(&found(2, 2)), Closes::Pane);
    assert_eq!(closes(&found(2, 1)), Closes::Window);
    assert_eq!(closes(&found(1, 1)), Closes::Session);
}

// ---------- hosts/win-prelude.test.ts: psmux ----------
// The first four check crates/inyon-engine/src/win-prelude.sh on its own, with no hub code: they are in
// inyon-engine's ops.rs, which runs the prelude. The sign-in one is in signin_tests.rs.

const PRELUDE: &str = include_str!("../../../inyon-engine/src/win-prelude.sh");

#[test]
fn an_agent_that_exits_on_windows_leaves_powershell_behind_and_remain_on_exit_is_set_on_the_window() {
    // a stand-in for psmux's tmux.exe: logs its argv
    let bin = TempDir::new("psmux-");
    let log = bin.path("argv");
    bin.write("tmux", &format!("#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> \"{}\"; done; echo >> \"{}\"\n", log.display(), log.display()));
    std::fs::set_permissions(bin.path("tmux"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin.write("argv", "");
    let cmd = format!("{}; {}; {}", remain_on_exit(true, "'%14.1'", true), respawn_shell(true, "'%14.1'", None), remain_on_exit(true, "'%14.1'", false));
    let path = format!("{}:/usr/bin:/bin", bin.s());
    sh(&format!("{PRELUDE}\n{cmd}"), None, &[("PATH", &path)]);
    assert_eq!(
        bin.read("argv"),
        "[set-option][-w][-t][$14:.%1][remain-on-exit][on]\n[respawn-pane][-k][-t][$14:.%1][powershell.exe]\n[set-option][-w][-t][$14:.%1][-u][remain-on-exit]\n"
    );
}

#[test]
fn a_paste_loads_its_buffer_into_the_panes_own_psmux_server_since_each_session_has_its_own() {
    let bin = TempDir::new("psmux-");
    let log = bin.path("argv");
    bin.write("tmux", &format!("#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> \"{}\"; done; echo >> \"{}\"\n", log.display(), log.display()));
    std::fs::set_permissions(bin.path("tmux"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let cmd = crate::tmux::tmux::paste_command("b1", "%14.1");
    let path = format!("{}:/usr/bin:/bin", bin.s());
    let tmp = Regex::new(r"\[[^\]]*tmp[^\]]*\]").unwrap();
    let go = |env: &[(&str, &str)]| {
        bin.write("argv", "");
        let mut all = vec![("PATH", path.as_str())];
        all.extend_from_slice(env);
        sh(&format!("{PRELUDE}\n{cmd}"), Some(b"x\ny"), &all);
        tmp.replace(&bin.read("argv"), "[file]").into_owned()
    };
    assert_eq!(go(&[("OS", "Windows_NT")]), "[-t][$14:.%1][load-buffer][-b][b1][file]\n[paste-buffer][-d][-p][-b][b1][-t][$14:.%1]\n");
    // tmux has one server, and no -t before the command
    assert_eq!(go(&[]), "[load-buffer][-b][b1][file]\n[paste-buffer][-d][-p][-b][b1][-t][$14:.%1]\n");
}

#[test]
fn typed_text_reaches_tmux_through_a_file_and_never_in_any_arguments() {
    let bin = TempDir::new("typed-");
    let log = bin.path("argv");
    // the fake tmux keeps its arguments, and the buffer it was handed
    bin.write(
        "tmux",
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> \"{0}\"; done; echo >> \"{0}\"\n[ \"$1\" = load-buffer ] && cat \"$4\" > \"{0}.buffer\"; exit 0\n",
            log.display()
        ),
    );
    std::fs::set_permissions(bin.path("tmux"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin.write("argv", "");
    let secret = "typed words that stay out of argv";
    let crate::tmux::tmux::TypeText::Keys { command, input } = crate::tmux::tmux::type_text("%1", secret, true, "b1") else { panic!() };
    let path = format!("{}:/usr/bin:/bin", bin.s());
    // sh -c <command> is what other users could read in /proc/*/cmdline, and so is every tmux run
    assert!(!command.contains(secret));
    sh(&command, Some(input.as_bytes()), &[("PATH", &path)]);
    let argv = Regex::new(r"\[[^\]]*tmp[^\]]*\]").unwrap().replace(&bin.read("argv"), "[file]").into_owned();
    assert_eq!(argv, "[load-buffer][-b][b1][file]\n[paste-buffer][-d][-b][b1][-t][%1]\n[send-keys][-t][%1][Enter]\n");
    assert_eq!(std::fs::read_to_string(bin.path("argv.buffer")).unwrap(), secret);
}

// ---------- taking turns per pane (review) ----------

#[test]
fn a_caller_that_stops_waiting_for_its_turn_never_lets_the_next_action_in_early() {
    run(|| async {
        let log = Rc::new(RefCell::new(Vec::<&str>::new()));
        let l = log.clone();
        let first = in_lane("h", "%1", move || async move {
            l.borrow_mut().push("first starts");
            sleep(1000).await;
            l.borrow_mut().push("first ends");
            Ok(())
        });
        // the second caller goes away while it waits (its HTTP client hung up)
        let l = log.clone();
        let mut second = Box::pin(in_lane("h", "%1", move || async move {
            l.borrow_mut().push("second");
            Ok(())
        }));
        assert!(futures_util::FutureExt::now_or_never(&mut second).is_none());
        drop(second);
        let l = log.clone();
        let third = in_lane("h", "%1", move || async move {
            l.borrow_mut().push("third");
            Ok(())
        });
        first.await.unwrap();
        third.await.unwrap();
        assert_eq!(*log.borrow(), ["first starts", "first ends", "second", "third"]);
    });
}

#[test]
fn a_send_whose_caller_went_away_is_still_typed_and_settled() {
    run(|| async {
        let b = fake_claude(vec![]);
        let mut send = Box::pin(send_message("local", "%9", "hello there", true, Some("gone"), false, false));
        assert!(futures_util::FutureExt::now_or_never(&mut send).is_none());
        drop(send);
        sleep(60_000).await;
        assert_eq!(b.borrow().sent, ["hello there"]);
        let w = watchers().get("local", "%9");
        let state = w.delivery("gone").map(|d| d.state);
        assert!(state.is_some_and(|s| s != SendStatus::Sending), "{state:?}");
        watchers().stop("local:%9");
    });
}

#[test]
fn a_tmux_session_is_renamed_by_its_number_with_what_tmux_wont_take_out_of_the_name() {
    run(|| async {
        let fake =
            Fake::install(|_, cmd, _| if cmd.contains("'taken'") { ExecResult { code: 1, stdout: String::new(), stderr: "duplicate session: taken".into() } } else { ok("") });
        pane("local", json!([]));
        assert_eq!(rename_tmux_session("local", "52", "  my  experiment ").await.unwrap(), "my experiment");
        assert_eq!(fake.cmds().last().unwrap(), "tmux rename-session -t '$52' -- 'my experiment'");
        // tmux turns . and : into _ itself; # would be a format
        assert_eq!(rename_tmux_session("local", "3", "api.v2:#(id)").await.unwrap(), "api_v2_-(id)");
        let e = rename_tmux_session("local", "3", "taken").await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(409), "There's already a tmux session called taken on that machine"));
        let n = fake.cmds().len();
        assert_eq!(rename_tmux_session("local", "3", "   ").await.unwrap_err().message, "Type a name");
        assert_eq!(rename_tmux_session("local", "$3; rm -rf ~", "x").await.unwrap_err().message, "Bad session");
        assert_eq!(fake.cmds().len(), n);
    });
}
