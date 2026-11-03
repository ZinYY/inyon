//! claude/integration.test.ts: the scripts run in a real shell against a temporary HOME, and
//! install and uninstall run through a machine whose shell sees only that HOME. The claudeStatus
//! tests are in agents/status.rs, resumeCommand's in claude/actions_tests.rs, and the restart
//! test reads a real process's arguments, so it's in inyon-backends.

use serde_json::{json, Value};

use super::*;
use crate::claude::integration_sh::{ACCOUNT_SH, MARKER_VERSION, MOVE_OLD_FOLDER_SH};
use crate::test_support::fake::{ok, Fake};
use crate::test_support::rt::run as run_rt;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{host, sh, TempDir};
use crate::tmux::tmux::{probe, HOOK_STATE_SH};

/// A ~/.claude.json shaped like the real one: other accountUuid keys come before oauthAccount.
fn write_config(home: &TempDir, account: &str) {
    let cfg = json!({
        "numStartups": 3,
        "projects": { "/x": { "lastAccount": { "accountUuid": "not-this-one" } } },
        "oauthAccount": { "accountUuid": account, "emailAddress": "a@b.c", "ccOnboardingFlags": { "x": true } },
    });
    home.write(".claude.json", &stringify_pretty(&cfg));
}

fn run(home: &TempDir, script: &str, input: &str, pane: &str) -> String {
    sh(script, Some(input.as_bytes()), &[("HOME", &home.s()), ("TMUX_PANE", pane)]).1
}

fn payload(session: &str) -> String {
    crate::js::stringify(&json!({
        "session_id": session,
        "model": { "id": "claude-opus", "display_name": "Opus" },
        "context_window": { "context_window_size": 1000, "used_percentage": 90, "current_usage": { "input_tokens": 900 } },
        "rate_limits": { "five_hour": { "used_percentage": 100, "resets_at": 4102444800u64 }, "seven_day": { "used_percentage": 93, "resets_at": 4102444800u64 } },
    }))
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn account_helper_reads_oauth_account_not_other_account_uuid_keys() {
    let home = TempDir::new("inyon-int-");
    write_config(&home, "acct-A");
    assert_eq!(run(&home, &format!(r#"{ACCOUNT_SH} "$HOME/.claude.json""#), "", "%9").trim(), "acct-A");
    assert_eq!(run(&home, &format!(r#"{ACCOUNT_SH} "$HOME/missing.json""#), "", "%9").trim(), "-");
}

#[test]
fn status_line_wrapper_saves_the_payload_and_the_account_it_was_written_under() {
    let home = TempDir::new("inyon-int-");
    write_config(&home, "acct-A");
    run(&home, STATUS_SCRIPT, &payload("s1"), "%9");
    assert_eq!(parse(&home.read(".inyon/status/9.json"))["session_id"], "s1");
    assert_eq!(home.read(".inyon/status/9.acct"), format!("acct-A {}/.claude.json\n", home.s()));
}

#[test]
fn hook_stamps_a_compaction_and_only_a_compaction() {
    let home = TempDir::new("inyon-int-");
    run(&home, HOOK_SCRIPT, r#"{"session_id":"s1","hook_event_name":"SessionStart","source":"startup"}"#, "%9");
    assert!(!home.exists(".inyon/panes/9.compact"));
    run(&home, HOOK_SCRIPT, r#"{"session_id":"s1","hook_event_name":"SessionStart","source":"compact"}"#, "%9");
    assert!(home.read(".inyon/panes/9.compact").trim().parse::<f64>().unwrap() > 0.0);
}

/// psmux numbers panes per session, so two sessions both run in %1; the hub calls them %<session>.<pane>
fn run_psmux(home: &TempDir, script: &str, input: &str, answer: &str) {
    let bin = TempDir::new("inyon-psmux-");
    bin.write("tmux", &format!("#!/bin/sh\n[ \"$*\" = \"display -p -t checkout #{{session_id}}\" ] && printf '{answer}'\n"));
    sh(&format!("chmod 755 '{}'", bin.path("tmux").display()), None, &[]);
    let path = format!("{}:{}", bin.s(), std::env::var("PATH").unwrap_or_default());
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(script).env_clear().env("HOME", home.s()).env("TMUX_PANE", "%1").env("PSMUX_SESSION", "checkout").env("PATH", path);
    c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    }
    child.wait().unwrap();
}

#[test]
fn psmux_the_status_line_and_hook_name_the_pane_as_the_hub_does_so_two_sessions_keep_their_own_files() {
    let home = TempDir::new("inyon-int-");
    write_config(&home, "acct-A");
    run_psmux(&home, STATUS_SCRIPT, &payload("s-psmux"), r"$36\r\n");
    assert_eq!(parse(&home.read(".inyon/status/36.1.json"))["session_id"], "s-psmux");
    run_psmux(&home, HOOK_SCRIPT, r#"{"session_id":"s-psmux","hook_event_name":"Stop"}"#, r"$36\n");
    assert_eq!(parse(&home.read(".inyon/panes/36.1.json"))["pane"], "%36.1");
}

#[test]
fn psmux_without_an_answer_from_psmux_the_pane_keeps_its_own_name() {
    let home = TempDir::new("inyon-int-");
    run_psmux(&home, STATUS_SCRIPT, &payload("s-plain"), "");
    assert_eq!(parse(&home.read(".inyon/status/1.json"))["session_id"], "s-plain");
}

#[test]
fn psmux_the_probe_reads_those_files_back_under_the_same_name() {
    run_rt(|| async {
        let out = ["@@STATUS".to_owned(), format!("36.1 1700000000 - - {}", payload("s-psmux")), "@@COMPACT".into(), "36.1 1700000001".into(), "@@END".into()].join("\n");
        Fake::install(move |_, _, _| ok(&out));
        let r = probe(&host("win", "local")).await.unwrap();
        assert_eq!(r.status_panes.get("%36.1").unwrap().data["session_id"], "s-psmux");
        assert_eq!(r.compact_panes.get("%36.1"), Some(&1700000001.0));
    });
}

fn settings_with_status(home: &TempDir, cmd: &str) {
    home.write(".claude/settings.json", &crate::js::stringify(&json!({ "statusLine": { "type": "command", "command": cmd } })));
}

#[test]
fn install_a_status_line_synced_from_windows_does_not_replace_the_one_that_runs_here() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        let orig = || home.read(".inyon/statusline.orig").trim().to_owned();
        settings_with_status(&home, "~/.claude/statusline.sh");
        install_integration(&h).await.unwrap();
        assert_eq!(orig(), "~/.claude/statusline.sh");
        settings_with_status(&home, "node C:/Users/me/.claude/statusline.js");
        install_integration(&h).await.unwrap();
        assert_eq!(orig(), "~/.claude/statusline.sh");
        assert_eq!(parse(&home.read(".claude/settings.json"))["statusLine"]["command"], "~/.inyon/statusline.sh");
        settings_with_status(&home, "~/bin/new-bar.sh");
        install_integration(&h).await.unwrap();
        assert_eq!(orig(), "~/bin/new-bar.sh");
    });
}

#[test]
fn install_a_saved_status_line_that_leads_back_to_the_wrapper_runs_once_instead_of_starting_itself_forever() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        install_integration(&h).await.unwrap();
        sh(&format!("ln -sfn '{}' '{}'", home.path(".inyon").display(), home.path(".tether").display()), None, &[]);
        home.write(".inyon/agy-statusline.sh", AGY_STATUS_SCRIPT);
        for (script, saved) in [("statusline.sh", "~/.tether/statusline.sh"), ("agy-statusline.sh", "~/.inyon/agy-statusline.sh")] {
            home.write(&format!(".inyon/{}", script.replace(".sh", ".orig")), &format!("{saved}\necho drawn\n"));
            let file = home.path(&format!(".inyon/{script}"));
            // a wrapper that started itself would never end: give up after 3 s
            let (code, out, _) = sh(&format!("timeout -s KILL 3 sh '{}'", file.display()), Some(payload("s1").as_bytes()), &[("HOME", &home.s()), ("TMUX_PANE", "%9")]);
            assert_eq!(code, 0, "{script}");
            assert_eq!(out.trim(), "drawn");
        }
        // healing the integration puts back the status line from before Inyon, or none
        home.write(".claude/settings.json.inyon-backup", r#"{"statusLine":{"type":"command","command":"bash ~/.claude/statusline.sh"}}"#);
        home.write(".inyon/statusline.orig", "~/.inyon/statusline.sh\n");
        install_integration(&h).await.unwrap();
        assert_eq!(home.read(".inyon/statusline.orig"), "bash ~/.claude/statusline.sh\n");
        home.write(".claude/settings.json.inyon-backup", r#"{"statusLine":{"type":"command","command":"node C:/Users/me/.claude/statusline.js"}}"#);
        home.write(".inyon/statusline.orig", "~/.tether/statusline.sh\n");
        install_integration(&h).await.unwrap();
        assert_eq!(home.read(".inyon/statusline.orig"), "");
    });
}

#[test]
fn installing_again_or_upgrading_from_tether_keeps_your_own_status_line() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        for wrapper in ["~/.inyon/statusline.sh", "~/.tether/statusline.sh"] {
            home.write(".claude/settings.json", &crate::js::stringify(&json!({ "statusLine": { "type": "command", "command": wrapper, "padding": 0 } })));
            home.write(".inyon/statusline.orig", "bash ~/.claude/statusline.sh\n");
            install_integration(&h).await.unwrap();
            assert_eq!(home.read(".inyon/statusline.orig"), "bash ~/.claude/statusline.sh\n");
            assert_eq!(parse(&home.read(".claude/settings.json"))["statusLine"], json!({ "type": "command", "command": "~/.inyon/statusline.sh", "padding": 0 }));
        }
        // and it is what the wrapper draws
        home.write(".inyon/statusline.orig", "echo my bar\n");
        let (_, out, _) = sh(&format!("sh '{}'", home.path(".inyon/statusline.sh").display()), Some(payload("s1").as_bytes()), &[("HOME", &home.s()), ("TMUX_PANE", "%9")]);
        assert_eq!(out.trim(), "my bar");
    });
}

#[test]
fn privacy_what_the_hook_and_status_line_save_is_readable_by_you_only_whatever_your_umask() {
    let home = TempDir::new("inyon-int-");
    run(&home, &format!("umask 022\n{HOOK_SCRIPT}"), r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"secret plan"}"#, "%9");
    assert_eq!(home.mode(".inyon"), 0o700);
    assert_eq!(home.mode(".inyon/panes"), 0o700);
    assert_eq!(home.mode(".inyon/events.jsonl"), 0o600);
    assert_eq!(home.mode(".inyon/panes/9.json"), 0o600);
    run(&home, &format!("umask 022\n{STATUS_SCRIPT}"), &payload("s1"), "%9");
    assert_eq!(home.mode(".inyon/status"), 0o700);
    assert_eq!(home.mode(".inyon/status/9.json"), 0o600);
    // your own status line still runs with your umask
    home.write(".inyon/statusline.orig", "touch \"$HOME/mine\"\n");
    run(&home, &format!("umask 022\n{STATUS_SCRIPT}"), &payload("s1"), "%9");
    assert_eq!(home.mode("mine"), 0o644);
}

#[test]
fn privacy_turning_the_integration_on_or_healing_it_closes_up_a_inyon_left_open() {
    let home = TempDir::new("inyon-int-");
    run(&home, HOOK_SCRIPT, r#"{"session_id":"s1","hook_event_name":"Stop"}"#, "%9");
    run_rt(|| async {
        Sandbox::install(&home.s());
        sh(&format!("chmod 755 '{0}/.inyon' && chmod 644 '{0}/.inyon/events.jsonl'", home.s()), None, &[]);
        install_integration(&host("local", "local")).await.unwrap();
        assert_eq!(home.mode(".inyon"), 0o700);
        assert_eq!(home.mode(".inyon/events.jsonl"), 0o600);
        assert_eq!(home.mode(".inyon/hook.sh"), 0o700);
    });
}

#[test]
fn reconnect_status_line_adds_it_to_project_settings_keeps_it_out_of_git_and_removes_it_on_uninstall() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        let proj = home.path("proj");
        let env = [("HOME", home.s())];
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        sh(&format!("git init -q '{}'", proj.display()), None, &env);
        home.write("proj/.claude/settings.local.json", r#"{"permissions":{"allow":["Bash(ls)"]}}"#);
        // the integration is on (the list of project files lives in its folder)
        install_integration(&h).await.unwrap();
        let p = proj.to_string_lossy().into_owned();
        reconnect_status_line(&h, &p).await.unwrap();
        reconnect_status_line(&h, &p).await.unwrap(); // idempotent
        let s = parse(&home.read("proj/.claude/settings.local.json"));
        assert_eq!(s["statusLine"]["command"], "~/.inyon/statusline.sh");
        assert_eq!(s["permissions"]["allow"], json!(["Bash(ls)"]));
        assert_eq!(home.read("proj/.git/info/exclude").matches("settings.local.json").count(), 1);
        assert_eq!(sh(&format!("cd '{p}' && git status --porcelain"), None, &env).1, "");

        home.write("other/.claude/settings.local.json", r#"{"statusLine":{"type":"command","command":"mine.sh"}}"#);
        let e = reconnect_status_line(&h, &home.path("other").to_string_lossy()).await.unwrap_err();
        assert!(e.message.contains("own status line"), "{}", e.message);

        uninstall_integration(&h).await.unwrap();
        let after = parse(&home.read("proj/.claude/settings.local.json"));
        assert!(after.get("statusLine").is_none());
        assert_eq!(after["permissions"]["allow"], json!(["Bash(ls)"]));
    });
}

#[test]
fn antigravity_installs_into_agy_settings_with_a_backup_chains_your_own_and_uninstall_restores_it() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        let file = ".gemini/antigravity-cli/settings.json";
        let read = || parse(&home.read(file));
        home.write(file, r#"{"model":"Gemini 3.8 Flash (High)","trustedWorkspaces":["/w"]}"#);

        // no status line of its own: keep agy's default one under ours
        install_integration(&h).await.unwrap();
        assert_eq!(read()["statusLine"], json!({ "type": "command", "command": "~/.inyon/agy-statusline.sh", "stack_with_default": true }));
        assert_eq!(read()["trustedWorkspaces"], json!(["/w"]));
        assert!(home.exists(&format!("{file}.inyon-backup")));
        assert_eq!(home.read(".inyon/agy-statusline.sh"), AGY_STATUS_SCRIPT);
        install_integration(&h).await.unwrap(); // idempotent
        assert_eq!(read()["statusLine"]["command"], "~/.inyon/agy-statusline.sh");
        uninstall_integration(&h).await.unwrap();
        assert!(read().get("statusLine").is_none());
        assert_eq!(read()["model"], "Gemini 3.8 Flash (High)");
        assert!(!home.exists(".inyon/agy-statusline.sh"));

        // your own status line keeps running behind the wrapper, and comes back on uninstall
        home.write(file, r#"{"statusLine":{"type":"command","command":"echo mine","padding":1}}"#);
        install_integration(&h).await.unwrap();
        assert_eq!(read()["statusLine"], json!({ "type": "command", "command": "~/.inyon/agy-statusline.sh", "padding": 1 }));
        let (_, out, _) = sh(
            &format!("sh '{}'", home.path(".inyon/agy-statusline.sh").display()),
            Some(b"{\"conversation_id\":\"c1\",\n\"agent_state\":\"idle\"}"),
            &[("HOME", &home.s()), ("TMUX_PANE", "%42")],
        );
        assert_eq!(out.trim(), "mine");
        assert_eq!(home.read(".inyon/agy-status/42.json"), r#"{"conversation_id":"c1","agent_state":"idle"}"#);
        uninstall_integration(&h).await.unwrap();
        assert_eq!(read()["statusLine"], json!({ "type": "command", "command": "echo mine", "padding": 1 }));
    });
}

#[test]
fn moving_an_install_from_before_the_rename_the_folder_moves_the_old_settings_count_as_outdated_and_installing_again_rewrites_them_once() {
    let old = TempDir::new("inyon-move-");
    let home = old.s();
    let shh = |cmd: &str| sh(cmd, None, &[("HOME", &home)]).1.trim().to_owned();
    // what Tether 0.6 left: its folder, and settings that name it next to your own hook and status line
    shh(
        r##"mkdir -p "$HOME/.tether/panes" "$HOME/.claude" && printf "#!/bin/sh\n" > "$HOME/.tether/hook.sh" && cp "$HOME/.tether/hook.sh" "$HOME/.tether/statusline.sh" && chmod 700 "$HOME/.tether/hook.sh" "$HOME/.tether/statusline.sh" && echo "~/bin/mybar.sh" > "$HOME/.tether/statusline.orig" && echo "{}" > "$HOME/.tether/events.jsonl""##,
    );
    old.write(
        ".claude/settings.json",
        &crate::js::stringify(&json!({
            "statusLine": { "type": "command", "command": "~/.tether/statusline.sh" },
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "~/.tether/hook.sh" }] }, { "hooks": [{ "type": "command", "command": "~/bin/notify.sh" }] }] },
        })),
    );
    shh(r#"mkdir -p "$HOME/.gemini/antigravity-cli" && echo "~/bin/agybar.sh" > "$HOME/.tether/agy-statusline.orig""#);
    old.write(".gemini/antigravity-cli/settings.json", r#"{"statusLine":{"type":"command","command":"~/.tether/agy-statusline.sh"}}"#);

    shh(MOVE_OLD_FOLDER_SH);
    assert_eq!(old.read(".inyon/events.jsonl"), "{}\n");
    // the old path still works for whatever points at it
    assert_eq!(shh(r#"[ -L "$HOME/.tether" ] && cat "$HOME/.tether/statusline.orig""#), "~/bin/mybar.sh");
    assert_eq!(shh(HOOK_STATE_SH), "outdated");
    // running it again changes nothing
    shh(MOVE_OLD_FOLDER_SH);
    assert_eq!(shh(r#"readlink "$HOME/.tether""#), old.path(".inyon").to_string_lossy());

    run_rt(|| async {
        Sandbox::install(&home);
        install_integration(&host("local", "local")).await.unwrap();
    });
    let settings = parse(&old.read(".claude/settings.json"));
    assert_eq!(settings["statusLine"]["command"], "~/.inyon/statusline.sh");
    // your status line is still the one it runs, not the old wrapper
    assert_eq!(old.read(".inyon/statusline.orig").trim(), "~/bin/mybar.sh");
    let stop: Vec<String> =
        settings["hooks"]["Stop"].as_array().unwrap().iter().flat_map(|g| g["hooks"].as_array().unwrap().iter().map(|h| h["command"].as_str().unwrap().to_owned())).collect();
    assert_eq!(stop.iter().filter(|c| c.contains("hook.sh")).collect::<Vec<_>>(), ["~/.inyon/hook.sh"]);
    assert!(stop.contains(&"~/bin/notify.sh".to_owned()));
    assert!(!crate::js::stringify(&settings).contains(".tether/"));
    assert_eq!(parse(&old.read(".gemini/antigravity-cli/settings.json"))["statusLine"]["command"], "~/.inyon/agy-statusline.sh");
    assert_eq!(old.read(".inyon/agy-statusline.orig").trim(), "~/bin/agybar.sh");
    assert_eq!(shh(HOOK_STATE_SH), "installed");
}

#[test]
fn scripts_from_a_newer_inyon_count_as_installed_so_an_older_app_leaves_them_alone() {
    let h = TempDir::new("inyon-version-");
    let home = h.s();
    let shh = |cmd: &str| sh(cmd, None, &[("HOME", &home)]).1.trim().to_owned();
    let scripts = |v: u32| {
        shh(&format!(
            r#"mkdir -p "$HOME/.inyon" "$HOME/.claude" && for f in hook statusline; do printf '#!/bin/sh\n# inyon-integration {v}\n' > "$HOME/.inyon/$f.sh"; chmod 700 "$HOME/.inyon/$f.sh"; done"#
        ))
    };
    scripts(MARKER_VERSION);
    h.write(".claude/settings.json", r#"{"statusLine":{"command":"~/.inyon/statusline.sh"},"hooks":{"Stop":[{"hooks":[{"command":"~/.inyon/hook.sh"}]}]}}"#);
    assert_eq!(shh(HOOK_STATE_SH), "installed");
    scripts(MARKER_VERSION + 1);
    assert_eq!(shh(HOOK_STATE_SH), "installed");
    scripts(MARKER_VERSION - 1);
    assert_eq!(shh(HOOK_STATE_SH), "outdated");
    // version 10 is newer than 9, not older because "1" sorts before "9"
    scripts(MARKER_VERSION + 4);
    assert_eq!(shh(HOOK_STATE_SH), "installed");
}

#[test]
fn settings_that_are_not_an_object_are_refused_as_invalid() {
    let home = TempDir::new("inyon-int-");
    home.write(".claude/settings.json", "[1]");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let e = install_integration(&host("local", "local")).await.unwrap_err();
        assert_eq!(e.status, Some(409));
    });
}

#[test]
fn stop_failure_is_only_registered_for_a_claude_code_that_has_it() {
    assert!(knows_stop_failure("2.1.309 (Claude Code)\n"));
    assert!(knows_stop_failure("2.1.78 (Claude Code)"));
    assert!(knows_stop_failure("3.0.0"));
    assert!(!knows_stop_failure("2.1.77 (Claude Code)"));
    assert!(!knows_stop_failure("1.0.120 (Claude Code)"));
    assert!(!knows_stop_failure(""));
    // before 2.1.101 an unknown event made Claude Code ignore the whole file
    for (version, want) in [("2.1.309 (Claude Code)", true), ("2.1.60 (Claude Code)", false)] {
        let home = TempDir::new("inyon-int-");
        run_rt(|| async {
            Sandbox::install_with(&home.s(), &format!("claude() {{ echo '{version}'; }}; "));
            install_integration(&host("local", "local")).await.unwrap();
            let hooks = &parse(&home.read(".claude/settings.json"))["hooks"];
            assert_eq!(hooks.get("StopFailure").is_some(), want, "{version}");
            assert!(hooks.get("Stop").is_some());
        });
    }
    // a Claude Code downgraded since: Inyon's StopFailure comes out again, yours stays
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install_with(&home.s(), "claude() { echo '2.1.309 (Claude Code)'; }; ");
        install_integration(&host("local", "local")).await.unwrap();
        let mut settings = parse(&home.read(".claude/settings.json"));
        settings["hooks"]["StopFailure"].as_array_mut().unwrap().push(json!({ "hooks": [{ "type": "command", "command": "~/bin/mine.sh" }] }));
        home.write(".claude/settings.json", &settings.to_string());
        Sandbox::install_with(&home.s(), "claude() { echo '2.1.60 (Claude Code)'; }; ");
        install_integration(&host("local", "local")).await.unwrap();
        let hooks = &parse(&home.read(".claude/settings.json"))["hooks"];
        assert_eq!(hooks["StopFailure"], json!([{ "hooks": [{ "type": "command", "command": "~/bin/mine.sh" }] }]));
        assert!(hooks.get("Stop").is_some());
    });
    // and a machine without claude on its path doesn't get it either
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install_with(&home.s(), "claude() { return 127; }; ");
        install_integration(&host("local", "local")).await.unwrap();
        assert!(parse(&home.read(".claude/settings.json"))["hooks"].get("StopFailure").is_none());
    });
}

/// A Claude Code window that moved its conversation to the background (`parkedJobId`) still shows
/// it; the background session runs outside tmux, so its hook has no pane of its own.
fn run_background(home: &TempDir, tmux: &str, input: &str) {
    home.write(
        ".claude/sessions/4242.json",
        r#"{"pid":4242,"sessionId":"b3a31007-70aa-42cc-9ed6-7c142f4a3624","kind":"interactive","tmux":"Find development:@1.%2","parkedJobId":"eef2ead0"}"#,
    );
    home.write(".claude/sessions/4343.json", r#"{"pid":4343,"sessionId":"eef2ead0-8d08-4b84-96ac-fca9d926eee6","kind":"bg","jobId":"eef2ead0"}"#);
    let bin = TempDir::new("inyon-tmux-");
    bin.write("tmux", tmux);
    sh(&format!("chmod 755 '{}'", bin.path("tmux").display()), None, &[]);
    let path = format!("{}:{}", bin.s(), std::env::var("PATH").unwrap_or_default());
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(HOOK_SCRIPT).env_clear().env("HOME", home.s()).env("PATH", path);
    c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    }
    child.wait().unwrap();
}

#[test]
fn a_background_sessions_events_go_to_the_window_that_moved_it_there() {
    let prompt = r#"{"session_id":"eef2ead0-8d08-4b84-96ac-fca9d926eee6","hook_event_name":"UserPromptSubmit","prompt":"don?"}"#;
    let home = TempDir::new("inyon-int-");
    run_background(&home, "#!/bin/sh\n[ \"$1\" = -V ] && echo 'tmux 3.4'\n", prompt);
    assert_eq!(parse(&home.read(".inyon/panes/2.json"))["data"]["prompt"], "don?");
    // psmux numbers panes per session: the hub's name for it has the session's id in it
    let home = TempDir::new("inyon-int-");
    run_background(
        &home,
        "#!/bin/sh\n[ \"$1\" = -V ] && echo 'tmux 3.3.8 psmux 3.3.8'\n[ \"$*\" = \"display -p -t Find development #{session_id}\" ] && printf '$55\\r\\n'\n",
        prompt,
    );
    assert_eq!(parse(&home.read(".inyon/panes/55.2.json"))["pane"], "%55.2");
    // psmux didn't say which session: no file under a name another session's pane may have
    let home = TempDir::new("inyon-int-");
    run_background(&home, "#!/bin/sh\n[ \"$1\" = -V ] && echo 'tmux 3.3.8 psmux 3.3.8'\n", prompt);
    assert!(!home.exists(".inyon/panes/2.json"));
    // a session nobody moved stays unrouted, as before
    let home = TempDir::new("inyon-int-");
    run_background(&home, "#!/bin/sh\necho 'tmux 3.4'\n", r#"{"session_id":"0000aaaa-8d08-4b84-96ac-fca9d926eee6","hook_event_name":"Stop"}"#);
    assert!(!home.exists(".inyon/panes/2.json"));
    assert!(home.read(".inyon/events.jsonl").contains(r#""pane":"""#));
}

/// Many keep ~/.claude/settings.json in their dotfiles, linked in. Installing replaced the link with
/// a file of its own, so the dotfiles stopped getting the change and Claude Code's next save went
/// to the copy.
#[test]
fn install_and_uninstall_keep_a_linked_settings_file_a_link() {
    let home = TempDir::new("inyon-int-");
    run_rt(|| async {
        Sandbox::install(&home.s());
        let h = host("local", "local");
        home.write("dotfiles/claude-settings.json", r#"{"model":"opus"}"#);
        std::fs::create_dir_all(format!("{}/.claude", home.s())).unwrap();
        std::os::unix::fs::symlink(format!("{}/dotfiles/claude-settings.json", home.s()), format!("{}/.claude/settings.json", home.s())).unwrap();
        let linked = || std::fs::symlink_metadata(format!("{}/.claude/settings.json", home.s())).unwrap().file_type().is_symlink();
        install_integration(&h).await.unwrap();
        assert!(linked());
        let s = parse(&home.read("dotfiles/claude-settings.json"));
        assert_eq!((s["model"].as_str(), s["statusLine"]["command"].as_str()), (Some("opus"), Some("~/.inyon/statusline.sh")));
        assert!(s["hooks"]["Stop"].is_array());
        uninstall_integration(&h).await.unwrap();
        assert!(linked());
        assert_eq!(parse(&home.read("dotfiles/claude-settings.json")), json!({ "model": "opus" }));
    });
}

/// Claude Code saves its settings while you work (a model you picked, a permission you allowed):
/// a write from what was read before that would put the old file back.
#[test]
fn settings_changed_since_they_were_read_are_not_written_over() {
    let home = TempDir::new("inyon-int-");
    let env = [("HOME", home.s())];
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    home.write(".claude/settings.json", r#"{"model":"opus"}"#);
    let read = sh(READ_SETTINGS, None, &env).1;
    let sum = read.lines().next().unwrap().to_owned();
    assert_eq!(read.split_once('\n').unwrap().1, r#"{"model":"opus"}"#);
    // Claude Code saves a switch meanwhile
    home.write(".claude/settings.json", r#"{"model":"sonnet"}"#);
    let (code, _, _) = sh(&write_settings_command(&sum), Some(br#"{"model":"opus","hooks":{}}"#), &env);
    assert_eq!(code, 3);
    assert_eq!(home.read(".claude/settings.json"), r#"{"model":"sonnet"}"#);
    // read again, it goes in
    let sum = sh(READ_SETTINGS, None, &env).1.lines().next().unwrap().to_owned();
    assert_eq!(sh(&write_settings_command(&sum), Some(br#"{"model":"sonnet","hooks":{}}"#), &env).0, 0);
    assert_eq!(home.read(".claude/settings.json"), r#"{"model":"sonnet","hooks":{}}"#);
    // a file that appeared after "none" was read isn't written over either
    std::fs::remove_file(format!("{}/.claude/settings.json", home.s())).unwrap();
    assert_eq!(sh(READ_SETTINGS, None, &env).1.trim(), "none");
    home.write(".claude/settings.json", "{}");
    assert_eq!(sh(&write_settings_command(""), Some(br#"{"a":1}"#), &env).0, 3);
}
