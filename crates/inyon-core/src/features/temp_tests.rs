//! features/temp.test.ts: the commands the hub runs, how it reads their answers, and the shell
//! that finds a machine's inyon, run for real.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use inyon_protocol::IntegrationState;
use serde_json::json;

use super::*;
use crate::test_support::fake::Fake;
use crate::test_support::rt::run as run_test;
use crate::test_support::{machine, sh, TempDir};
use crate::tmux::tmux::{ProbeResult, TmuxProbe};

/// A machine called Box in the registry; connected when its probe found a home folder.
fn put(id: &str, connected: bool) {
    let rt = machine(id, "node", json!([]));
    rt.borrow_mut().hold_polls();
    if connected {
        rt.borrow_mut().probe = Some(Rc::new(RefCell::new(ProbeResult {
            hostname: "box".into(),
            home: "/home/me".into(),
            os: "Linux".into(),
            now: 0.0,
            latency_ms: 0.0,
            tmux: TmuxProbe::Ok,
            integration: IntegrationState::Missing,
            sessions: vec![],
            agent_procs: IndexMap::new(),
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
        })));
    }
}

fn res(code: i32, stdout: &str, stderr: &str) -> ExecResult {
    ExecResult { code, stdout: stdout.into(), stderr: stderr.into() }
}

#[test]
fn the_list_is_the_machines_own_inyon_temp_ls() {
    run_test(|| async {
        put("box", true);
        let fake = Fake::install(|_, _, _| res(0, "{\"v\":1,\"ok\":true,\"host\":\"studio\",\"roots\":[],\"items\":[]}\n", ""));
        assert_eq!(temp_list("box").await.unwrap(), json!({ "v": 1, "ok": true, "host": "studio", "roots": [], "items": [] }));
        assert_eq!(fake.cmds(), [format!("{INYON} ls --json")]);
    });
}

#[test]
fn paths_go_quoted_after_two_dashes_and_previews_stay_under_the_limit() {
    run_test(|| async {
        put("box", true);
        let fake = Fake::install(|_, _, _| res(0, "{\"v\":1,\"ok\":true}", ""));
        temp_files("box", "/tmp/claude-1000/it's here").await.unwrap();
        temp_preview("box", "/tmp/claude-1000/a.png", Some(1_500_000.0)).await.unwrap();
        temp_preview("box", "/tmp/claude-1000/a.png", Some(1e12)).await.unwrap();
        temp_preview("box", "/tmp/claude-1000/a.png", None).await.unwrap();
        assert_eq!(
            fake.cmds(),
            [
                format!("{INYON} files --json -- '/tmp/claude-1000/it'\\''s here'"),
                format!("{INYON} show --json --max 1500000 -- '/tmp/claude-1000/a.png'"),
                format!("{INYON} show --json --max 8388608 -- '/tmp/claude-1000/a.png'"),
                format!("{INYON} show --json --max 8388608 -- '/tmp/claude-1000/a.png'"),
            ]
        );
        // as the Bun hub: below 1 byte is 1 byte; 0, a fraction of a byte or not a number is the
        // whole limit
        for (max, sent) in [(-5.0, 1), (f64::NEG_INFINITY, 1), (0.5, PREVIEW_MAX), (0.0, PREVIEW_MAX), (f64::NAN, PREVIEW_MAX), (f64::INFINITY, PREVIEW_MAX)] {
            temp_preview("box", "/tmp/claude-1000/a.png", Some(max)).await.unwrap();
            let cmd = fake.cmds().pop().unwrap();
            assert!(cmd.contains(&format!("--max {sent} ")), "{max} sent {cmd}");
        }
        // each well under the 120 s the server lets a quiet connection sit
        assert!(fake.calls.borrow().iter().all(|c| c.timeout_ms == Some(WAIT)));
        assert_eq!(temp_files("box", "").await.unwrap_err().message, "Which item?");
    });
}

#[test]
fn deleting_sends_the_paths_on_stdin_and_returns_what_stayed() {
    run_test(|| async {
        put("box", true);
        let fake = Fake::install(|_, _, _| res(1, r#"{"v":1,"ok":false,"removed":[],"freed":0,"failed":[["/etc","outside"]]}"#, ""));
        let r = temp_delete("box", Some(&json!(["/etc"]))).await.unwrap();
        assert_eq!(r, json!({ "v": 1, "ok": true, "removed": [], "freed": 0, "failed": [["/etc", "outside"]] }));
        let call = fake.calls.borrow()[0].clone();
        assert_eq!((call.cmd, call.input.as_deref()), (format!("{INYON} rm --stdin --json"), Some(r#"["/etc"]"#)));
        assert_eq!(call.timeout_ms, Some(DELETE_WAIT));
        for bad in [None, Some(json!([])), Some(json!(["ok", 3])), Some(json!([""])), Some(json!("x"))] {
            assert_eq!(temp_delete("box", bad.as_ref()).await.unwrap_err().message, "Send the paths to delete");
        }
    });
}

#[test]
fn unknown_or_offline_machines_and_refusals_get_their_status() {
    run_test(|| async {
        put("box", true);
        put("away", false);
        Fake::install(|_, _, _| res(1, r#"{"v":1,"ok":false,"error":"That isn't inside one of the temp folders Inyon lists"}"#, ""));
        assert_eq!(temp_list("nope").await.unwrap_err(), http(404, "Unknown machine"));
        assert_eq!(temp_list("away").await.unwrap_err(), http(503, "Machine is not connected yet"));
        assert_eq!(temp_preview("box", "/etc/passwd", None).await.unwrap_err(), http(400, "That isn't inside one of the temp folders Inyon lists"));
    });
}

#[test]
fn an_inyon_too_old_for_temp_or_none_at_all_says_what_to_do() {
    let old = res(2, "", "error: unrecognized subcommand 'temp'\n\nUsage: inyon [OPTIONS] [COMMAND]\n");
    assert_eq!(read_answer("Studio", &old).unwrap_err(), http(409, "Update Inyon on Studio to see its temp files"));
    assert_eq!(read_answer("Studio", &res(2, "", "inyon-engine 0.12.0: the engine behind inyon\n")).unwrap_err(), http(409, too_old("Studio")));
    assert_eq!(read_answer("Studio", &res(127, "", "inyon: not found\n")).unwrap_err(), http(409, "Install Inyon on Studio to see its temp files"));
    assert_eq!(read_answer("Studio", &res(124, "", "timed out")).unwrap_err(), http(504, "Studio took too long to answer"));
    assert_eq!(read_answer("Studio", &res(1, "", "something broke\n")).unwrap_err(), http(500, "something broke"));
    assert_eq!(read_answer("Studio", &res(0, "noise\n{\"v\":1}\n", "")).unwrap(), json!({ "v": 1 }).as_object().unwrap().clone());
}

#[test]
fn the_shell_finds_the_machines_inyon_and_passes_the_arguments_through() {
    let dir = TempDir::new("inyon-temp-sh-");
    let fake = |rel: &str, tag: &str| {
        let d = dir.path(rel);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("inyon");
        std::fs::write(&p, format!("#!/bin/sh\nprintf '{tag}'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p.to_string_lossy().into_owned()
    };
    let home = dir.path("home").to_string_lossy().into_owned();
    let cmd = format!("{INYON} files --json -- '/tmp/a b'");
    let run = |env: &[(&str, &str)]| {
        let mut all = vec![("HOME", home.as_str()), ("PATH", "/usr/bin:/bin")];
        all.extend_from_slice(env);
        sh(&cmd, None, &all)
    };
    assert_eq!(run(&[]), (127, String::new(), "inyon: not found\n".into()));
    fake("home/.inyon/bin", "home");
    assert_eq!(run(&[]).1, "home [temp] [files] [--json] [--] [/tmp/a b]");
    fake("path", "path");
    let path = format!("{}:/usr/bin:/bin", dir.path("path").display());
    assert_eq!(run(&[("PATH", &path)]).1, "path [temp] [files] [--json] [--] [/tmp/a b]");
    let exe = fake("exe", "exe");
    assert_eq!(run(&[("PATH", &path), ("INYON_EXE", &exe), ("INYON_SCRIPT", "/opt/inyon.ts")]).1, "exe [/opt/inyon.ts] [temp] [files] [--json] [--] [/tmp/a b]");
    assert_eq!(run(&[("INYON_EXE", &exe), ("INYON_SCRIPT", "")]).1, "exe [temp] [files] [--json] [--] [/tmp/a b]");
}
