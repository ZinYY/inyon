use super::*;
use serde_json::json;

fn agent(command: &str, like: AgentKind) -> CustomAgent {
    CustomAgent { id: "x".into(), name: "x".into(), command: command.into(), like }
}

#[test]
fn a_process_is_the_custom_agent_when_its_program_and_flags_match() {
    let yolo = agent("claude --dangerously-skip-permissions", AgentKind::Claude);
    assert!(yolo.matches("claude", "claude --dangerously-skip-permissions --model opus"));
    // the full path, and flags in another order
    assert!(yolo.matches("claude", "/home/a/.local/bin/claude --model opus --dangerously-skip-permissions"));
    // plain claude is the built-in agent, not this one
    assert!(!yolo.matches("claude", "claude --model opus"));

    let oc = agent("openclaude", AgentKind::Claude);
    // ps cuts comm at 15 characters; argv[0] still names it
    assert!(oc.matches("openclaude", "openclaude"));
    assert!(oc.matches("node", "node /home/a/.npm-global/bin/openclaude --verbose"));
    assert!(oc.matches("openclaude.exe", "C:\\tools\\openclaude.exe"));
    // a shell running a wrapper isn't the agent: its child is
    assert!(!oc.matches("bash", "bash /home/a/bin/openclaude"));
    assert!(!oc.matches("vim", "vim openclaude.md"));
}

#[test]
fn the_saved_list_is_checked_whole() {
    let list = parse_list(&json!([
        { "name": " Claude YOLO ", "command": "claude --dangerously-skip-permissions", "like": "claude" },
        { "name": "codex-oss", "command": "codex --oss", "like": "codex", "id": "ignored" },
    ]))
    .unwrap();
    assert_eq!(list[0], CustomAgent { id: "claude-yolo".into(), name: "Claude YOLO".into(), command: "claude --dangerously-skip-permissions".into(), like: AgentKind::Claude });
    assert_eq!(list[1].id, "codex-oss");

    let bad = |v: Value| parse_list(&v).unwrap_err();
    assert_eq!(bad(json!({})), "Custom agents must be a list");
    assert_eq!(bad(json!([{ "name": "", "command": "x", "like": "claude" }])), "A custom agent needs a name of 1 to 40 characters");
    assert_eq!(bad(json!([{ "name": "a", "command": "x\ny", "like": "claude" }])), "a: the command must be one line of 1 to 300 characters");
    assert_eq!(bad(json!([{ "name": "a", "command": "x", "like": "vim" }])), "a: pick the agent it works like");
    assert_eq!(bad(json!([{ "name": "!!", "command": "x", "like": "claude" }])), "!!: use at least one letter or digit in the name");
    assert_eq!(bad(json!([{ "name": "A b", "command": "x", "like": "claude" }, { "name": "a-B", "command": "y", "like": "codex" }])), "Two custom agents are called a-B");
    assert_eq!(bad(Value::Array(vec![json!({ "name": "a", "command": "x", "like": "claude" }); 25])), "At most 24 custom agents");
}

#[test]
fn detection_reads_only_the_loaded_list() {
    set(vec![]);
    assert_eq!(detect("openclaude", "openclaude"), None);
    set(vec![agent("openclaude", AgentKind::Claude)]);
    assert_eq!(detect("openclaude", "openclaude").map(|a| a.like), Some(AgentKind::Claude));
    set(vec![]);
}
