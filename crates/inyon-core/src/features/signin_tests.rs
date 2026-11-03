//! features/signin.test.ts

use super::*;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::machine;
use crate::test_support::rt::run;

fn v(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

#[test]
fn opencode_api_key_prompt_exposes_a_secret_input_without_a_login_url() {
    assert_eq!(
        parse_open_code_auth_input(&v(&["◇ Login method", "│ Manually enter API Key", "◆ Enter your API key", "│ _"])),
        Some(SignInInput { label: "Enter your API key".into(), secret: true })
    );
    assert_eq!(parse_open_code_auth_input(&v(&["◆ Login method"])), None);
    assert_eq!(parse_open_code_auth_input(&v(&["◇ Enter your API key", "│ Done"])), None);
}

#[test]
fn device_code_sign_in_screen() {
    let s = parse_signin(
        "
Welcome to Codex

1. Open this link in your browser and sign in to your account
   https://auth.openai.com/codex/device

2. Enter this one-time code (expires in 15 minutes)
   K7QD-9XWPM

",
    );
    assert_eq!(s.links, vec!["https://auth.openai.com/codex/device"]);
    assert_eq!(s.code.as_deref(), Some("K7QD-9XWPM"));
    assert_eq!(s.exit, None);
    assert_eq!(s.lines[0], "Welcome to Codex");
}

#[test]
fn finished_sign_in_with_a_long_link_joined_by_capture_j() {
    let s = parse_signin(
        "Browser didn't open? Use the url below to sign in (c to copy)

https://claude.ai/oauth/authorize?code=true&client_id=abc&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&state=XYZ.

Paste code here if prompted >
Login successful.

[inyon-exit 0]
",
    );
    assert_eq!(s.links, vec!["https://claude.ai/oauth/authorize?code=true&client_id=abc&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&state=XYZ"]);
    assert_eq!(s.code, None);
    assert_eq!(s.exit, Some(0));
    assert_eq!(s.lines.last().unwrap(), "Login successful.");
}

#[test]
fn agy_draws_its_sign_in_link_across_lines_of_its_own_they_are_joined_back() {
    let s = parse_signin(
        " Open the URL below in your browser:
 ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=12345.apps.googleusercontent.com&code_challenge=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopq
 rstuvwxyz&code_challenge_method=S256&state=abc
 ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 After authenticating, copy the code displayed in the browser and paste it below:
 authorization code...",
    );
    assert_eq!(s.links, vec!["https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=12345.apps.googleusercontent.com&code_challenge=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz&code_challenge_method=S256&state=abc"]);
}

fn option(n: u64, label: &str, selected: bool) -> PromptOption {
    PromptOption { n, label: label.into(), detail: None, selected, checked: None, current: None }
}

#[test]
fn opencode_clack_provider_radio_list_supports_navigation_and_ignores_search_and_footer_rows() {
    let menu = parse_open_code_auth_menu(&v(&[
        "┌ Add credential",
        "◆ Select provider",
        "│ Search: _",
        "│ ● OpenCode Zen (recommended)",
        "│ ○ OpenAI",
        "│ ○ GitHub Copilot",
        "│ ...",
        "│ ↑/↓ to select • Enter: confirm • Type: to search",
    ]))
    .unwrap();
    assert_eq!(menu.navigate, Some(true));
    assert_eq!(menu.options, vec![option(1, "OpenCode Zen (recommended)", true), option(2, "OpenAI", false), option(3, "GitHub Copilot", false)]);
    assert_eq!(
        serde_json::to_value(&menu.options).unwrap(),
        serde_json::json!([{ "n": 1, "label": "OpenCode Zen (recommended)", "selected": true }, { "n": 2, "label": "OpenAI", "selected": false }, { "n": 3, "label": "GitHub Copilot", "selected": false }])
    );
    assert_eq!(menu.body, vec!["Select provider"]);
}

#[test]
fn opencode_next_login_method_prompt_replaces_the_completed_provider_choice() {
    let menu = parse_open_code_auth_menu(&v(&[
        "◇ Select provider",
        "│ OpenAI",
        "◆ Login method",
        "│ ○ ChatGPT Pro/Plus (browser)",
        "│ ● ChatGPT Pro/Plus (headless)",
        "│ ○ Manually enter API Key",
    ]))
    .unwrap();
    assert_eq!(menu.body, vec!["Login method"]);
    assert_eq!(menu.options.iter().find(|o| o.selected).map(|o| o.n), Some(2));
    assert_eq!(parse_open_code_auth_menu(&v(&["◇ Login method", "│ API Key", "◆ Enter API Key", "│ _"])), None);
    assert_eq!(parse_open_code_auth_menu(&v(&["◇ Finished", "│ ○ Earlier option"])), None);
}

/// From win-prelude.test.ts.
#[test]
fn a_sign_in_on_windows_is_a_powershell_line_that_prints_the_exit_mark() {
    assert_eq!(sign_in_windows(AgentKind::Gemini, None), r#"$env:NO_BROWSER='true'; gemini; Write-Output ''; Write-Output "[inyon-exit $([int]$LASTEXITCODE)]""#);
    assert!(sign_in_windows(AgentKind::Codex, None).starts_with("codex login --device-auth; "));
}

#[test]
fn connecting_a_provider_runs_opencodes_own_login_for_that_provider_and_its_error_counts_as_a_failure_even_at_exit_0() {
    run(|| async {
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        let f = Fake::install(|_, _, _| ok("%9\n"));
        assert_eq!(start_signin("h", "opencode", Some("anthropic")).await.unwrap(), SignInStart { pane: "%9".into() });
        let ran = f.cmds();
        // quoted once for sh -c and once for tmux, so the provider id sits between escaped quotes
        assert!(regex::Regex::new(r"opencode auth login -p \S*anthropic\S*; s=\$\?").unwrap().is_match(&ran[0]), "{}", ran[0]);
        assert!(ran[0].contains("inyon-login-opencode-"));
        assert_eq!(start_signin("h", "opencode", Some("bad id")).await.unwrap_err(), HubError::http(400, "Unknown provider"));
        assert!(sign_in_windows(AgentKind::Opencode, Some("groq")).starts_with("opencode auth login -p 'groq'; "));
        assert!(sign_in_windows(AgentKind::Opencode, None).starts_with("opencode auth login; "));
        let failed = parse_signin("┌  Add credential\nError: Unknown provider \"nope\"\n\n[inyon-exit 0]\n");
        assert_eq!(failed.exit, Some(1));
        assert_eq!(failed.error.as_deref(), Some("Unknown provider \"nope\""));
        let done = parse_signin("┌  Add credential\n│\n◇  Enter your API key\n│  ▪▪▪▪\n│\n└  Done\n\n[inyon-exit 0]\n");
        assert_eq!(done.exit, Some(0));
        assert_eq!(done.error, None);
    });
}

#[test]
fn stopping_a_sign_in_ends_that_session_by_its_exact_name_even_if_its_pane_has_just_gone() {
    run(|| async {
        let name = format!("{LOGIN_PREFIX}claude-k7q4");
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        let displays = std::rc::Rc::new(std::cell::Cell::new(0));
        let n = name.clone();
        let f = Fake::install(move |_, cmd, _| {
            // the pane is there for the ownership check, and gone by the time anything asks again
            if cmd.contains("display") {
                displays.set(displays.get() + 1);
                if displays.get() == 1 {
                    return ok(&format!("{n}\n"));
                }
            }
            ok("")
        });
        stop_signin("h", "%7").await.unwrap();
        let kill = f.cmds().into_iter().find(|c| c.contains("kill-session")).unwrap();
        assert!(kill.contains(&format!("'={name}'")));
        // never an empty target, which tmux reads as the most recent session
        assert!(!kill.contains("$("));
    });
}

// ---------- beyond the TS tests ----------

#[test]
fn a_pane_that_isnt_a_sign_in_is_refused() {
    run(|| async {
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        Fake::install(|_, _, _| ok("work\n"));
        assert_eq!(read_signin("h", "%3").await.unwrap_err(), HubError::http(404, "That sign-in has ended"));
        assert_eq!(read_signin("h", "3").await.unwrap_err(), HubError::http(400, "Bad pane"));
        assert_eq!(start_signin("h", "nope", None).await.unwrap_err(), HubError::http(400, "Unknown agent"));
    });
}

#[test]
fn a_finished_screen_serializes_as_the_ts_one_does() {
    let s = parse_signin("Go to https://x.io/a).\nCode ABCD-1234\n[inyon-exit 2]\n");
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::json!({ "lines": ["Go to https://x.io/a).", "Code ABCD-1234"], "links": ["https://x.io/a"], "code": "ABCD-1234", "exit": 2 })
    );
}
