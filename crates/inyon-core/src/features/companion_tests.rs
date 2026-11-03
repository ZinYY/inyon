use inyon_protocol::{AgentKind, ChatItem, PromptOption, SessionInfo};
use serde_json::{json, Value};

use super::*;

fn sessions(v: Value) -> Vec<SessionInfo> {
    serde_json::from_value(v).unwrap()
}

fn pane(id: &str, agent: Option<&str>) -> Value {
    let mut p = json!({ "id": id, "index": 0, "active": true, "command": "x", "cwd": "/", "title": "", "width": 80, "height": 24, "dead": false });
    if let Some(k) = agent {
        p["agent"] = json!({ "kind": k, "pid": 1, "state": "idle" });
    }
    p
}

fn session(name: &str, panes: Vec<Value>) -> Value {
    json!({ "id": "$1", "name": name, "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": panes }] })
}

fn marked(s: &[SessionInfo]) -> Vec<String> {
    s.iter().flat_map(|s| s.windows.iter().flat_map(|w| w.panes.iter())).filter(|p| p.companion == Some(true)).map(|p| p.id.clone()).collect()
}

#[test]
fn only_the_first_agent_pane_of_the_companion_session_on_the_hubs_machine_is_marked() {
    let all = || {
        sessions(json!([session("main", vec![pane("%1", Some("claude"))]), session("companion", vec![pane("%2", None), pane("%3", Some("claude")), pane("%4", Some("codex"))])]))
    };
    let mut s = all();
    mark("local", &mut s, true);
    assert_eq!(marked(&s), ["%3"]);
    // turned off, or another machine's session of that name: nothing
    let mut s = all();
    mark("local", &mut s, false);
    assert!(marked(&s).is_empty());
    let mut s = all();
    mark("laptop", &mut s, true);
    assert!(marked(&s).is_empty());
    // a mark from an earlier poll goes once the agent has quit
    let mut s = all();
    mark("local", &mut s, true);
    for p in s[1].windows[0].panes.iter_mut() {
        p.agent = None;
    }
    mark("local", &mut s, true);
    assert!(marked(&s).is_empty());
    assert!(s[1].windows[0].panes.iter().all(|p| p.companion.is_none()));
}

fn opts(labels: &[&str]) -> Vec<PromptOption> {
    labels.iter().enumerate().map(|(i, l)| PromptOption { n: i as u64 + 1, label: (*l).into(), detail: None, selected: false, checked: None, current: None }).collect()
}

#[test]
fn a_choice_is_picked_by_its_number_or_its_words() {
    let o = opts(&["Yes", "Yes, and don't ask again for git push", "No, and tell Claude what to do differently"]);
    assert_eq!(pick_choice(&o, "2").unwrap().0, 2);
    assert_eq!(pick_choice(&o, " yes ").unwrap().0, 1);
    assert_eq!(pick_choice(&o, "no").unwrap().0, 3);
    assert_eq!(pick_choice(&o, "don't ask").unwrap().0, 2);
    // more than one starts with it, or nothing matches: the choices are listed
    let o2 = opts(&["Yes, once", "Yes, always"]);
    let e = pick_choice(&o2, "yes").unwrap_err();
    assert!(e.contains("More than one") && e.contains("1. Yes, once, 2. Yes, always"), "{e}");
    assert!(pick_choice(&o, "4").unwrap_err().contains("no choice 4"));
    assert!(pick_choice(&o, "maybe").unwrap_err().contains("no choice like \"maybe\""));
    assert!(pick_choice(&o, "  ").unwrap_err().starts_with("Name a choice"));
}

// "inyon answer … yes trust" was refused for Claude Code's "Yes, I trust this folder"
#[test]
fn a_choice_is_picked_by_its_words_in_any_place() {
    let trust = opts(&["No, exit", "Yes, I trust this folder"]);
    assert_eq!(pick_choice(&trust, "yes trust").unwrap(), (2, "Yes, I trust this folder".into()));
    assert_eq!(pick_choice(&trust, "Trust, yes").unwrap().0, 2);
    // whole words only, and every one of them
    assert!(pick_choice(&trust, "yes exit").is_err());
    assert!(pick_choice(&trust, "trus folder").is_err());
    let o = opts(&["Yes, allow once", "Yes, allow always"]);
    assert!(pick_choice(&o, "allow yes").unwrap_err().contains("More than one"));
    // an address, a path or a flag is one word: another host's approval is never picked
    let curl = opts(&["Yes", "Yes, and don't ask again for commands that start with curl -sI https://example.com", "No"]);
    assert!(pick_choice(&curl, "don't ask curl -sI https://com.example").is_err());
    assert!(pick_choice(&curl, "don't ask curl -s https://example.com").is_err());
    assert_eq!(pick_choice(&curl, "https://example.com don't ask").unwrap().0, 2);
    let path = opts(&["Yes, allow edits in src/app", "Yes, allow edits in app/src"]);
    assert_eq!(pick_choice(&path, "allow app/src").unwrap().0, 2);
}

fn items(v: Value) -> Vec<ChatItem> {
    serde_json::from_value(v).unwrap()
}

#[test]
fn a_conversation_reads_as_who_said_what_with_the_last_entries_kept() {
    let base = |seq: i64| json!({ "seq": seq, "uuid": format!("u{seq}"), "ts": "" });
    let mut list = vec![];
    let mut add = |extra: Value| {
        let mut v = base(list.len() as i64);
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        list.push(v);
    };
    add(json!({ "kind": "user", "text": "port the **parser**", "images": 0 }));
    add(json!({ "kind": "assistant", "msgId": "m", "block": { "type": "thinking", "text": "hmm" } }));
    add(json!({ "kind": "assistant", "msgId": "m", "block": { "type": "tool_use", "id": "t", "name": "Bash", "input": { "command": "cargo test\n-q" } } }));
    add(json!({ "kind": "tool_result", "toolUseId": "t", "isError": false, "text": "ok" }));
    add(json!({ "kind": "tool_result", "toolUseId": "t", "isError": true, "text": "boom" }));
    add(json!({ "kind": "assistant", "msgId": "m", "block": { "type": "text", "text": "## Done\n- `parse` works" } }));
    add(json!({ "kind": "turn_end", "durationMs": 5 }));
    let out = condense(&items(Value::Array(list)), 20);
    assert_eq!(
        out,
        vec![
            json!({ "role": "you", "text": "port the parser" }),
            json!({ "role": "tool", "text": "Bash: cargo test -q" }),
            json!({ "role": "error", "text": "boom" }),
            json!({ "role": "agent", "text": "Done\nparse works" }),
        ]
    );
}

#[test]
fn condense_keeps_only_the_last_entries_asked_for() {
    let list: Vec<Value> = (0..5).map(|i| json!({ "kind": "user", "seq": i, "uuid": "u", "ts": "", "text": format!("m{i}"), "images": 0 })).collect();
    let out = condense(&items(Value::Array(list)), 2);
    assert_eq!(out, vec![json!({ "role": "you", "text": "m3" }), json!({ "role": "you", "text": "m4" })]);
}

#[test]
fn the_instructions_start_with_the_line_that_lets_inyon_rewrite_them() {
    let text = instructions();
    assert_eq!(text.lines().next(), Some(MARK));
    // start() looks for this phrase in the first line
    assert!(MARK.contains("Inyon rewrites this file"));
    for cmd in ["inyon sessions", "inyon read", "inyon send", "inyon answer"] {
        assert!(text.contains(cmd), "{cmd}");
    }
    // reading runs without asking; sending and answering ask first
    let allow = claude_settings()["permissions"]["allow"].clone();
    assert!(allow.as_array().unwrap().iter().all(|a| !a.as_str().unwrap().contains("send") && !a.as_str().unwrap().contains("answer")));
    assert!(launch_flags(AgentKind::Codex, true).iter().any(|f| f.contains("network_access=true")));
    assert!(launch_flags(AgentKind::Claude, false).is_empty());
    // without its rule, Codex asks before anything that isn't known to be safe
    assert!(!launch_flags(AgentKind::Codex, true).iter().any(|f| f.contains("ask-for-approval")));
    assert!(launch_flags(AgentKind::Codex, false).contains(&"--ask-for-approval untrusted".to_string()));
}

/// Codex once ran `inyon send` without asking: its sandbox lets the command reach the hub. The
/// script writes a rule that asks, and trusts the folder so Codex reads it, without breaking a
/// config.toml that has its own say about the folder.
#[cfg(unix)]
#[test]
fn codex_gets_a_rule_that_asks_before_a_send_and_a_trusted_folder() {
    let home = std::env::temp_dir().join(format!("inyon-companion-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let cfg = home.join(".codex/config.toml");
    let run = || {
        let mut c = std::process::Command::new("sh")
            .args(["-c", &codex_setup_script()])
            .env("HOME", &home)
            .env_remove("CODEX_HOME")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(c.stdin.as_mut().unwrap(), CODEX_RULES.as_bytes()).unwrap();
        drop(c.stdin.take());
        String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap().trim().to_string()
    };
    let dir = format!("{}/.inyon/companion", home.display());
    let table = format!("[projects.\"{dir}\"]");

    // no config.toml yet: the folder is trusted, and the rule is in place
    assert_eq!(run(), "trusted");
    assert_eq!(std::fs::read_to_string(format!("{dir}/.codex/rules/companion.rules")).unwrap(), CODEX_RULES);
    assert!(CODEX_RULES.contains(r#"["inyon", ["send", "answer"]], decision = "prompt""#));
    let first = std::fs::read_to_string(&cfg).unwrap();
    assert!(first.contains(&format!("{table}\ntrust_level = \"trusted\"")));
    // a second start adds nothing
    assert_eq!(run(), "trusted");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), first);

    // you marked the folder untrusted yourself: that stays, and the companion asks for everything
    std::fs::write(&cfg, format!("model = \"x\"\n\n{table}\ntrust_level = \"untrusted\"\n\n[other]\ntrust_level = \"trusted\"\n")).unwrap();
    assert_eq!(run(), "");
    assert!(std::fs::read_to_string(&cfg).unwrap().contains("\"untrusted\""));
    // trusted among other settings of yours
    std::fs::write(&cfg, format!("model = \"x\"\n{table}\nfoo = 1\ntrust_level = \"trusted\"\n")).unwrap();
    assert_eq!(run(), "trusted");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn the_agent_setting_is_read_once_and_written_through() {
    crate::test_support::rt::run(|| async {
        crate::platform::store::kv::set_value("settings", &json!({ "companion": { "agent": "codex" } })).unwrap();
        AGENT.with(|a| *a.borrow_mut() = None);
        assert_eq!(agent(), Some(AgentKind::Codex));
        set_agent(None).unwrap();
        assert_eq!(agent(), None);
        assert!(crate::platform::store::kv::get_value("settings").unwrap().get("companion").is_none());
        set_agent(Some(AgentKind::Claude)).unwrap();
        AGENT.with(|a| *a.borrow_mut() = None);
        assert_eq!(agent(), Some(AgentKind::Claude));
        assert_eq!(status()["agent"], json!("claude"));
    });
}
