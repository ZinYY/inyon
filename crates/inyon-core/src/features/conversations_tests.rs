//! features/conversations.test.ts

use std::rc::Rc;

use inyon_protocol::AgentKind::{Antigravity, Claude, Codex, Copilot, Opencode};
use serde_json::{json, Value};

use super::*;
use crate::hosts::names::set_session_name;
use crate::platform::rt::{spawn_task, Deferred};
use crate::test_support::rt::run;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{host, machine, sh, TempDir};

const CWD: &str = "/work/app";
const OTHER: &str = "/work/other";

fn id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn day() -> u64 {
    crate::js::date::parse("2026-10-12T00:00:00Z") as u64 / 1000
}

/// Writes a file and sets when it was last written, in seconds.
fn file(home: &TempDir, path: &str, lines: &[Value], mtime: u64) {
    let text: Vec<String> = lines.iter().map(|l| l.as_str().map(str::to_owned).unwrap_or_else(|| l.to_string())).collect();
    home.write(path, &(text.join("\n") + "\n"));
    let f = std::fs::File::options().write(true).open(home.path(path)).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime)).unwrap();
}

fn user_line(text: Value, extra: Value) -> Value {
    let mut l = json!({ "type": "user" });
    for (k, v) in extra.as_object().unwrap() {
        l[k] = v.clone();
    }
    l["message"] = json!({ "role": "user", "content": text });
    l
}

fn sqlite(db: &std::path::Path, sql: &str) {
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let (code, _, err) = sh(&format!("sqlite3 '{}'", db.display()), Some(sql.as_bytes()), &[]);
    assert_eq!(code, 0, "{err}");
}

/// A throwaway home with each agent's files in it: the real ~/.claude, ~/.codex and the rest are
/// never read. Every machine's shell runs with it as HOME.
fn fixture() -> TempDir {
    let home = TempDir::new("inyon-convos-");
    let d = day();
    let none = json!({});

    // Claude Code
    let dir = ".claude/projects/-work-app";
    let f = |n: u32| format!("{dir}/{}.jsonl", id(n));
    file(
        &home,
        &f(1),
        &[
            json!({ "type": "mode" }),
            user_line(json!("Fix the login page"), none.clone()),
            json!({ "type": "ai-title", "aiTitle": "Login fix" }),
            json!({ "type": "custom-title", "customTitle": "Auth work" }),
            json!({ "type": "ai-title", "aiTitle": "Login fix again" }),
        ],
        d + 300,
    );
    file(
        &home,
        &f(2),
        &[
            user_line(json!("<local-command-caveat>Caveat: ignore</local-command-caveat>"), none.clone()),
            user_line(json!("<command-name>/model</command-name>"), none.clone()),
            user_line(json!([{ "type": "text", "text": "Write   the\nREADME" }]), none.clone()),
            json!({ "type": "ai-title", "aiTitle": "Readme draft" }),
        ],
        d + 200,
    );
    file(&home, &f(3), &[json!({ "type": "mode" }), user_line(json!("ignored"), json!({ "isMeta": true })), user_line(json!("Plan the \"release\" notes"), none.clone())], d + 100);
    // opened and closed without a word: nothing to resume
    file(&home, &f(4), &[json!({ "type": "mode" }), json!({ "type": "permission-mode" })], d + 400);
    file(&home, &format!(".claude/projects/-work-other/{}.jsonl", id(5)), &[user_line(json!("Elsewhere"), none.clone())], d + 500);

    // Codex
    let meta = |n: u32, cwd: &str, source: Value| json!({ "timestamp": "2026-10-12T00:00:00Z", "type": "session_meta", "payload": { "id": id(n), "cwd": cwd, "source": source, "instructions": "x".repeat(2000) } });
    let msg = |text: &str| json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] } });
    let env = msg("<environment_context>\n  <cwd>/work/app</cwd>\n</environment_context>");
    let roll = |n: u32, seg: &str| format!(".codex/sessions/2026/10/12/rollout-2026-10-12T00-00-00-{}{seg}.jsonl", id(n));
    file(&home, &roll(10, ""), &[meta(10, CWD, json!("cli")), env.clone(), msg("Add a dark mode"), msg("later")], d + 50);
    file(&home, &roll(11, ""), &[meta(11, CWD, json!("vscode")), env.clone(), msg("Speed up the build")], d + 60);
    // the same thread continued in a segment file after a rewind
    file(&home, &roll(11, &format!("_{}", id(99))), &[meta(11, CWD, json!("vscode")), msg("Rewound")], d + 90);
    file(&home, &roll(12, ""), &[meta(12, CWD, json!("exec")), msg("From a script")], d + 70);
    file(&home, &roll(13, ""), &[meta(13, CWD, json!({ "subagent": { "thread_spawn": {} } })), msg("Subagent work")], d + 80);
    file(&home, &roll(14, ""), &[meta(14, OTHER, json!("cli")), msg("Other folder")], d + 85);
    file(&home, &roll(15, ""), &[meta(15, CWD, json!("cli")), env], d + 95);
    file(&home, ".codex/session_index.jsonl", &[json!({ "id": id(10), "thread_name": "Dark mode" }), json!({ "id": id(10), "thread_name": "Dark mode toggle" })], d);

    // GitHub Copilot
    let s = ".copilot/session-state";
    let start = json!({ "type": "session.start", "data": { "context": { "cwd": CWD } } });
    let ws = |n: u32, lines: &[String], mtime: u64, events: &[Value]| {
        let lines: Vec<Value> = lines.iter().map(|l| json!(l)).collect();
        file(&home, &format!("{s}/{}/workspace.yaml", id(n)), &lines, mtime);
        file(&home, &format!("{s}/{}/events.jsonl", id(n)), events, mtime);
    };
    ws(20, &[format!("id: {}", id(20)), format!("cwd: {CWD}"), "name: Fix flaky tests".into(), "user_named: false".into()], d + 20, std::slice::from_ref(&start));
    ws(21, &[format!("id: {}", id(21)), format!("cwd: \"{CWD}\""), "name: 'It''s quoted'".into()], d + 30, std::slice::from_ref(&start));
    ws(22, &[format!("id: {}", id(22)), format!("cwd: {CWD}")], d + 40, &[start.clone(), json!({ "type": "user.message", "data": { "content": "No name yet" } })]);
    ws(23, &[format!("id: {}", id(23)), format!("cwd: {OTHER}"), "name: Elsewhere".into()], d + 50, std::slice::from_ref(&start));
    // before workspace.yaml: the first event says where it ran
    file(&home, &format!("{s}/{}/events.jsonl", id(24)), &[start.clone(), json!({ "type": "user.message", "data": { "content": "Old style" } })], d + 10);

    // OpenCode
    sqlite(
        &home.path(".local/share/opencode/opencode.db"),
        &format!(
            "create table session (id text primary key, title text, directory text, parent_id text, time_created integer, time_updated integer, agent text, model text, time_archived integer);
insert into session values ('ses_new', 'Refactor parser', '{CWD}', null, 1, 2000, null, null, null);
insert into session values ('ses_old', 'Write tests', '{CWD}', null, 1, 1000, null, null, null);
insert into session values ('ses_child', 'Subtask', '{CWD}', 'ses_new', 1, 3000, null, null, null);
insert into session values ('ses_gone', 'Archived', '{CWD}', null, 1, 4000, null, null, 5);
insert into session values ('ses_far', 'Other', '{OTHER}', null, 1, 5000, null, null, null);"
        ),
    );

    // Antigravity CLI
    let uris = |dirs: &[&str]| json!(dirs.iter().map(|d| format!("file://{d}")).collect::<Vec<_>>()).to_string();
    let row = |n: u32, title: &str, preview: &str, t: &str, w: String, parent: &str, depth: u32| {
        format!("insert into conversation_summaries values ('{}', '{title}', '{preview}', '{t}', '{w}', '{parent}', {depth});", id(n))
    };
    sqlite(
        &home.path(".gemini/antigravity-cli/conversation_summaries.db"),
        &[
            "create table conversation_summaries (conversation_id text primary key, title text not null default '', preview text not null default '', last_modified_time datetime not null, workspace_uris text not null, parent_conversation_id text not null default '', nesting_depth integer not null default 0);".to_owned(),
            row(30, "Ship the CLI", "ship it", "2026-10-11 08:22:33.43735957+00:00", uris(&["/home/u/.inyon/uploads", CWD]), "", 0),
            row(31, "", "Only a preview", "2026-10-10 10:00:00.5+00:00", uris(&[CWD]), "", 0),
            row(32, "Child", "", "2026-10-12 10:00:00+00:00", uris(&[CWD]), &id(30), 1),
            row(33, "Prefix folder", "", "2026-10-12 11:00:00+00:00", uris(&[&format!("{CWD}-old")]), "", 0),
            row(34, "Spaces", "", "2026-10-12 12:00:00+00:00", json!(["file:///work/my%20app"]).to_string(), "", 0),
        ]
        .join("\n"),
    );

    Sandbox::install(&home.s());
    home
}

fn convos() -> Host {
    host("convos", "local")
}

fn conv(id: String, title: &str, at: u64) -> PastConversation {
    PastConversation { id, title: title.into(), at, current: None }
}

// ---------- Claude Code ----------

#[test]
fn claude_lists_the_folder_newest_first_with_rename_over_the_made_up_title_over_the_first_prompt() {
    let _home = fixture();
    run(|| async {
        let d = day();
        assert_eq!(
            list_conversations(&convos(), Claude, CWD, LIMIT).await.unwrap(),
            [conv(id(1), "Auth work", (d + 300) * 1000), conv(id(2), "Readme draft", (d + 200) * 1000), conv(id(3), "Plan the \"release\" notes", (d + 100) * 1000)]
        );
    });
}

#[test]
fn claude_a_folder_with_no_transcripts_has_none() {
    let _home = fixture();
    run(|| async {
        assert_eq!(list_conversations(&convos(), Claude, "/nowhere", LIMIT).await.unwrap(), []);
    });
}

// ---------- Codex ----------

#[test]
fn codex_lists_the_folders_threads_from_its_terminal_and_editor_named_as_codex_names_them() {
    let _home = fixture();
    run(|| async {
        let d = day();
        assert_eq!(
            list_conversations(&convos(), Codex, CWD, LIMIT).await.unwrap(),
            [conv(id(11), "Speed up the build", (d + 90) * 1000), conv(id(10), "Dark mode toggle", (d + 50) * 1000)]
        );
    });
}

// ---------- GitHub Copilot ----------

#[test]
fn copilot_lists_the_folders_sessions_by_their_names_or_the_first_prompt() {
    let _home = fixture();
    run(|| async {
        let list = list_conversations(&convos(), Copilot, CWD, LIMIT).await.unwrap();
        let got: Vec<(String, String)> = list.into_iter().map(|c| (c.id, c.title)).collect();
        let want: Vec<(String, String)> =
            [(22, "No name yet"), (21, "It's quoted"), (20, "Fix flaky tests"), (24, "Old style")].iter().map(|(n, t)| (id(*n), t.to_string())).collect();
        assert_eq!(got, want);
    });
}

// ---------- OpenCode ----------

#[test]
fn opencode_lists_the_folders_top_level_unarchived_sessions() {
    let _home = fixture();
    run(|| async {
        assert_eq!(
            list_conversations(&convos(), Opencode, CWD, LIMIT).await.unwrap(),
            [conv("ses_new".into(), "Refactor parser", 2000), conv("ses_old".into(), "Write tests", 1000)]
        );
    });
}

// ---------- Antigravity CLI ----------

#[test]
fn antigravity_lists_the_folders_conversations_not_subagents_or_a_folder_sharing_its_names_start() {
    let _home = fixture();
    run(|| async {
        let at = |s: &str| crate::js::date::parse(s) as u64;
        assert_eq!(
            list_conversations(&convos(), Antigravity, CWD, LIMIT).await.unwrap(),
            [conv(id(30), "Ship the CLI", at("2026-10-11T08:22:33.437Z")), conv(id(31), "Only a preview", at("2026-10-10T10:00:00.5Z"))]
        );
    });
}

#[test]
fn antigravity_folders_written_as_uris_match_their_plain_path() {
    let _home = fixture();
    run(|| async {
        let titles: Vec<String> = list_conversations(&convos(), Antigravity, "/work/my app", LIMIT).await.unwrap().into_iter().map(|c| c.title).collect();
        assert_eq!(titles, ["Spaces"]);
    });
}

#[test]
fn agys_times_parse() {
    assert_eq!(agy_time(Some(&json!("2026-10-11 08:22:33.43735957+00:00"))), crate::js::date::parse("2026-10-11T08:22:33.437Z") as u64);
    assert_eq!(agy_time(Some(&json!("nonsense"))), 0);
}

// ---------- reading lines cut short on the machine ----------

#[test]
fn a_json_string_stops_at_its_end_or_where_the_line_was_cut() {
    assert_eq!(json_string_at("\"a\\\"b\\nc\" tail", 1), "a\"b\nc");
    assert_eq!(json_string_at("\"caf\\u00e9 and more", 1), "café and more");
    // a cut in the middle of an escape drops the broken part
    assert_eq!(json_string_at("\"half \\u00", 1), "half ");
    assert_eq!(json_string_at("\"ends with \\", 1), "ends with ");
}

#[test]
fn titles_are_one_line_without_the_tags_agents_wrap_prompts_in() {
    assert_eq!(one_line("<system-reminder>hidden</system-reminder>\n Fix  the\tbug "), "Fix the bug");
    assert_eq!(one_line(&"x".repeat(500)).len(), 200);
}

#[test]
fn yaml_scalars() {
    assert_eq!(yaml_scalar(" plain words "), "plain words");
    assert_eq!(yaml_scalar(" \"a: \\\"b\\\"\" "), "a: \"b\"");
    assert_eq!(yaml_scalar(" 'it''s' "), "it's");
}

// ---------- a pane ----------

/// The machine "convos" with one pane, %7, working in CWD, and `agent` running in it.
fn pane(agent: Option<Value>) {
    let mut p = json!({ "id": "%7", "cwd": CWD });
    if let Some(mut a) = agent {
        a["pid"] = json!(1);
        a["state"] = json!("idle");
        p["agent"] = a;
    }
    machine("convos", "local", json!([p]));
}

#[test]
fn a_pane_lists_the_running_agents_conversations_marks_the_open_one_and_shows_names_given_in_inyon() {
    let _home = fixture();
    run(|| async {
        pane(Some(json!({ "kind": "copilot", "sessionId": id(21) })));
        set_session_name("convos", &id(20), "Named in Inyon", None, Some(Copilot));
        let r = pane_conversations("convos", "%7", Some(&json!("claude"))).await.unwrap();
        assert_eq!(r.agent, Copilot);
        assert_eq!(r.conversations.iter().find(|c| c.id == id(21)).unwrap().current, Some(true));
        assert_eq!(r.conversations.iter().find(|c| c.id == id(20)).unwrap().title, "Named in Inyon");
        assert_eq!(r.conversations.iter().filter(|c| c.current == Some(true)).count(), 1);
    });
}

#[test]
fn a_pane_with_no_agent_running_lists_the_one_asked_for() {
    let _home = fixture();
    run(|| async {
        pane(None);
        let ids: Vec<String> = pane_conversations("convos", "%7", Some(&json!("claude"))).await.unwrap().conversations.into_iter().map(|c| c.id).collect();
        assert_eq!(ids, [id(1), id(2), id(3)]);
        assert!(pane_conversations("convos", "%7", None).await.unwrap_err().message.contains("Say which agent"));
        assert!(pane_conversations("convos", "%7", Some(&json!("nope"))).await.unwrap_err().message.contains("Unknown agent"));
    });
}

#[test]
fn a_pane_with_an_agent_inyon_cannot_list_says_so() {
    let _home = fixture();
    run(|| async {
        pane(Some(json!({ "kind": "gemini" })));
        assert!(pane_conversations("convos", "%7", None).await.unwrap_err().message.contains("can't list Gemini"));
    });
}

// ---------- switching ----------

type Calls = Rc<RefCell<Vec<String>>>;

/// Steps that only note what they were asked to do.
fn noting() -> Calls {
    let calls: Calls = Rc::default();
    let note = |name: &'static str, with_agent: bool| -> steps::Step {
        let calls = calls.clone();
        Rc::new(move |_h: &str, p: &str, kind: AgentKind, id: &str| {
            calls.borrow_mut().push(if with_agent { format!("{name} {p} {} {id}", kind.as_str()) } else { format!("{name} {p}") });
            Box::pin(async { Ok(()) })
        })
    };
    steps::EXIT.with(|s| *s.borrow_mut() = Some(note("exit", false)));
    steps::READY.with(|s| *s.borrow_mut() = Some(note("ready", false)));
    steps::START.with(|s| *s.borrow_mut() = Some(note("start", true)));
    calls
}

fn running_claude() {
    pane(Some(json!({ "kind": "claude", "sessionId": id(1) })));
}

#[test]
fn switching_exits_the_running_agent_waits_for_the_shell_then_resumes_the_other_conversation() {
    run(|| async {
        let calls = noting();
        running_claude();
        assert_eq!(switch_conversation("convos", "%7", Some(&json!("claude")), Some(&json!(id(2)))).await.unwrap(), Switched { switched: true });
        assert_eq!(*calls.borrow(), ["exit %7".to_owned(), "ready %7".into(), format!("start %7 claude {}", id(2))]);
    });
}

#[test]
fn switching_with_nothing_running_just_starts_it() {
    run(|| async {
        let calls = noting();
        pane(None);
        switch_conversation("convos", "%7", Some(&json!("codex")), Some(&json!(id(10)))).await.unwrap();
        assert_eq!(*calls.borrow(), [format!("start %7 codex {}", id(10))]);
    });
}

#[test]
fn switching_to_the_open_conversation_leaves_it_as_it_is() {
    run(|| async {
        let calls = noting();
        running_claude();
        assert_eq!(switch_conversation("convos", "%7", Some(&json!("claude")), Some(&json!(id(1)))).await.unwrap(), Switched { switched: false });
        assert!(calls.borrow().is_empty());
    });
}

#[test]
fn switching_refuses_an_id_a_shell_could_read_as_more_and_another_agent() {
    run(|| async {
        let calls = noting();
        running_claude();
        let claude = json!("claude");
        assert_eq!(switch_conversation("convos", "%7", Some(&claude), Some(&json!(format!("{}; rm -rf ~", id(2))))).await.unwrap_err().message, "Bad conversation id");
        assert_eq!(switch_conversation("convos", "%7", Some(&claude), Some(&json!(""))).await.unwrap_err().message, "Bad conversation id");
        assert_eq!(switch_conversation("convos", "%7", Some(&json!("codex")), Some(&json!(id(2)))).await.unwrap_err().status, Some(409));
        assert!(calls.borrow().is_empty());
    });
}

#[test]
fn switching_a_second_time_while_one_is_under_way_is_refused() {
    run(|| async {
        noting();
        running_claude();
        let release: Deferred<()> = Deferred::new();
        let held = release.clone();
        steps::EXIT.with(|s| {
            *s.borrow_mut() = Some(Rc::new(move |_: &str, _: &str, _: AgentKind, _: &str| -> futures_util::future::LocalBoxFuture<'static, HubResult<()>> {
                let held = held.clone();
                Box::pin(async move {
                    held.wait().await;
                    Ok(())
                })
            }))
        });
        let first = spawn_task(async { switch_conversation("convos", "%7", Some(&json!("claude")), Some(&json!(id(2)))).await });
        crate::test_support::rt::wait(0).await;
        assert_eq!(switch_conversation("convos", "%7", Some(&json!("claude")), Some(&json!(id(3)))).await.unwrap_err().status, Some(409));
        release.resolve(());
        assert_eq!(first.wait().await.unwrap(), Switched { switched: true });
    });
}

#[test]
fn switching_fails_when_the_agent_wont_exit_and_nothing_is_started() {
    run(|| async {
        let calls = noting();
        running_claude();
        steps::EXIT.with(|s| {
            *s.borrow_mut() = Some(Rc::new(|_: &str, _: &str, _: AgentKind, _: &str| -> futures_util::future::LocalBoxFuture<'static, HubResult<()>> {
                Box::pin(async { Err(http(504, "Claude Code did not exit")) })
            }))
        });
        assert!(switch_conversation("convos", "%7", Some(&json!("claude")), Some(&json!(id(2)))).await.unwrap_err().message.contains("did not exit"));
        assert!(calls.borrow().is_empty());
    });
}
