use serde_json::{json, Value};

use super::*;
use crate::claude::integration::HOOK_SCRIPT;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::rt::run as run_rt;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{host, sh, TempDir};
use crate::tmux::tmux::{probe, PROBE};

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// The probe's lines that list the agents running Inyon's hook, run against `home`.
fn hooked_lines(home: &TempDir) -> Vec<String> {
    let start = PROBE.find("echo \"@@HOOKED\"\n").unwrap() + "echo \"@@HOOKED\"\n".len();
    let end = start + PROBE[start..].find("echo \"@@HOOK\"").unwrap();
    let out = sh(&format!("{}true", &PROBE[start..end]), None, &[("HOME", &home.s())]).1;
    out.lines().map(str::to_owned).collect()
}

#[test]
fn copilots_file_runs_the_hook_for_each_event_and_never_for_a_permission_it_would_decide() {
    let v = copilot_hooks();
    assert_eq!(v["version"], 1);
    let hooks = v["hooks"].as_object().unwrap();
    assert_eq!(hooks.keys().map(String::as_str).collect::<Vec<_>>(), COPILOT_EVENTS);
    assert_eq!(hooks["agentStop"], json!([{ "type": "command", "bash": "test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh copilot agentStop || echo '{}'", "timeoutSec": 10 }]));
    assert!(!hooks.contains_key("permissionRequest") && !hooks.contains_key("preToolUse"));
}

#[test]
fn gemini_keeps_your_own_hooks_and_adds_inyons_once() {
    let mine = json!({ "matcher": "write_file", "hooks": [{ "name": "check", "type": "command", "command": "./check.sh" }] });
    let settings = obj(json!({ "theme": "Dark", "hooks": { "AfterAgent": [mine.clone()] } }));
    let once = with_gemini_hooks(settings.clone()).unwrap();
    assert_eq!(once["theme"], "Dark");
    assert_eq!(once["hooks"]["AfterAgent"][0], mine);
    assert_eq!(once["hooks"]["AfterAgent"][1]["hooks"][0]["command"], "test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh gemini AfterAgent || echo '{}'");
    assert_eq!(once["hooks"]["BeforeAgent"].as_array().unwrap().len(), 1);
    assert_eq!(with_gemini_hooks(once.clone()).unwrap(), once);
    // and out again, leaving yours
    let out = without_gemini_hooks(once).unwrap();
    assert_eq!(Value::Object(out), json!({ "theme": "Dark", "hooks": { "AfterAgent": [mine] } }));
    assert_eq!(without_gemini_hooks(settings), None);
    assert!(with_gemini_hooks(obj(json!({ "hooks": [] }))).is_err());
}

#[test]
fn agy_gets_a_named_entry_of_its_own_beside_yours() {
    let yours = json!({ "PostToolUse": [{ "matcher": "run_command", "hooks": [{ "command": "./lint.sh" }] }] });
    let next = with_agy_hooks(obj(json!({ "lint": yours.clone() })));
    assert_eq!(next["lint"], yours);
    assert_eq!(next["inyon"]["Stop"], json!([{ "type": "command", "command": "test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh antigravity Stop || echo '{}'", "timeout": 10 }]));
    assert_eq!(next["inyon"]["PreInvocation"][0]["command"], "test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh antigravity PreInvocation || echo '{}'");
}

#[test]
fn installing_adds_the_hook_to_each_agent_on_the_machine_and_uninstalling_takes_only_it_out() {
    let home = TempDir::new("inyon-hooks-");
    home.write(".gemini/settings.json", &crate::js::stringify(&json!({ "theme": "Dark", "hooks": { "AfterAgent": [{ "hooks": [{ "command": "./mine.sh" }] }] } })));
    home.write(".gemini/config/hooks.json", &crate::js::stringify(&json!({ "lint": { "Stop": [{ "command": "./lint.sh" }] } })));
    run_rt(|| async {
        Sandbox::install_with(&home.s(), "copilot() { :; }; gemini() { :; }; agy() { :; }; ");
        let h = host("local", "local");
        assert_eq!(hooked_lines(&home), Vec::<String>::new());
        assert_eq!(install_agent_hooks(&h).await.unwrap(), HOOKED_AGENTS);
        assert_eq!(parse(&home.read(".copilot/hooks/inyon.json")), copilot_hooks());
        let gemini = parse(&home.read(".gemini/settings.json"));
        assert_eq!(gemini["hooks"]["AfterAgent"][0]["hooks"][0]["command"], "./mine.sh");
        assert_eq!(gemini["hooks"]["AfterAgent"][1]["hooks"][0]["command"], "test -x ~/.inyon/hook.sh && ~/.inyon/hook.sh gemini AfterAgent || echo '{}'");
        let agy = parse(&home.read(".gemini/config/hooks.json"));
        assert_eq!(agy["lint"]["Stop"][0]["command"], "./lint.sh");
        assert!(agy["inyon"].is_object());
        // the shared files were backed up first, as they were
        assert_eq!(parse(&home.read(".gemini/settings.json.inyon-backup"))["hooks"]["AfterAgent"].as_array().unwrap().len(), 1);
        assert!(home.exists(".gemini/config/hooks.json.inyon-backup"));
        // the probe sees all three
        assert_eq!(hooked_lines(&home), ["copilot", "gemini", "antigravity"]);
        // installing again changes nothing
        let before = home.read(".gemini/settings.json");
        install_agent_hooks(&h).await.unwrap();
        assert_eq!(home.read(".gemini/settings.json"), before);

        uninstall_agent_hooks(&h).await.unwrap();
        assert!(!home.exists(".copilot/hooks/inyon.json"));
        assert_eq!(parse(&home.read(".gemini/settings.json")), json!({ "theme": "Dark", "hooks": { "AfterAgent": [{ "hooks": [{ "command": "./mine.sh" }] }] } }));
        assert_eq!(parse(&home.read(".gemini/config/hooks.json")), json!({ "lint": { "Stop": [{ "command": "./lint.sh" }] } }));
        assert_eq!(hooked_lines(&home), Vec::<String>::new());
    });
}

#[test]
fn an_agent_that_isnt_there_gets_nothing_and_broken_settings_stop_only_that_agent() {
    let home = TempDir::new("inyon-hooks-");
    home.write(".gemini/settings.json", "{ not json");
    run_rt(|| async {
        // gemini is there with broken settings; copilot and agy aren't there at all
        Sandbox::install_with(&home.s(), "gemini() { :; }; copilot() { return 127; }; agy() { return 127; }; command() { [ \"$2\" = gemini ]; }; ");
        assert_eq!(install_agent_hooks(&host("local", "local")).await.unwrap(), Vec::<AgentKind>::new());
        assert_eq!(home.read(".gemini/settings.json"), "{ not json");
        assert!(!home.exists(".copilot") && !home.exists(".gemini/config"));
    });
}

#[test]
fn the_hook_tags_another_agents_event_answers_it_and_keeps_claude_codes_lines_as_they_were() {
    let home = TempDir::new("inyon-hooks-");
    let run = |args: &str, input: &str| sh(&format!("set -- {args}\n{HOOK_SCRIPT}"), Some(input.as_bytes()), &[("HOME", &home.s()), ("TMUX_PANE", "%5")]).1;
    let out = run("copilot agentStop", "{\"sessionId\":\"c1\",\n\"transcriptPath\":\"/t\"}");
    assert_eq!(out.trim(), "{}");
    let line = parse(&home.read(".inyon/panes/5.json"));
    assert_eq!((line["agent"].as_str(), line["event"].as_str(), line["data"]["sessionId"].as_str()), (Some("copilot"), Some("agentStop"), Some("c1")));
    // what it can't be: a name with odd characters is cut down to what's allowed
    run("'co$pilot;' 'agent Stop'", r#"{"sessionId":"c2"}"#);
    let line = parse(&home.read(".inyon/panes/5.json"));
    assert_eq!((line["agent"].as_str(), line["event"].as_str()), (Some("copilot"), Some("agentStop")));
    // Claude Code's: no answer, no agent
    let out = run("", r#"{"session_id":"s1","hook_event_name":"Stop"}"#);
    assert_eq!(out, "");
    let line = parse(&home.read(".inyon/panes/5.json"));
    assert!(line.get("agent").is_none() && line["data"]["hook_event_name"] == "Stop");
    assert_eq!(home.read(".inyon/events.jsonl").lines().count(), 3);
}

#[test]
fn an_agents_entry_runs_the_hook_while_it_is_there_and_answers_on_its_own_once_it_is_gone() {
    let home = TempDir::new("inyon-hooks-");
    let cmd = hook_command(AgentKind::Copilot, "agentStop");
    let run = || sh(&cmd, Some(br#"{"sessionId":"c1"}"#), &[("HOME", &home.s()), ("TMUX_PANE", "%7")]);
    // no ~/.inyon/hook.sh (Inyon removed, say): `{}`, a clean exit, nothing written
    let (code, out, _) = run();
    assert_eq!((code, out.trim()), (0, "{}"));
    assert!(!std::path::Path::new(&home.s()).join(".inyon/panes/7.json").exists());
    std::fs::create_dir_all(std::path::Path::new(&home.s()).join(".inyon")).unwrap();
    let script = std::path::Path::new(&home.s()).join(".inyon/hook.sh");
    std::fs::write(&script, HOOK_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (code, out, _) = run();
    assert_eq!((code, out.trim()), (0, "{}"));
    assert_eq!(parse(&home.read(".inyon/panes/7.json"))["event"], "agentStop");
}

#[test]
fn the_probe_reads_which_agents_have_the_hook() {
    run_rt(|| async {
        Fake::install(|_, _, _| ok("@@HOOKED\ncopilot\nantigravity\nsomething-else\n@@END\n"));
        let r = probe(&host("box", "ssh")).await.unwrap();
        assert_eq!(r.hooked, [AgentKind::Copilot, AgentKind::Antigravity]);
    });
}
