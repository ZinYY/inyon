//! The AGENTS catalog: agents.json reads into the Rust types without losing anything, and the
//! helpers behave like their TypeScript counterparts in agents.ts.

use inyon_protocol::*;

#[test]
fn every_agent_is_in_the_catalog_in_order() {
    assert_eq!(agents().keys().copied().collect::<Vec<_>>(), AGENT_KINDS);
    for (kind, def) in agents() {
        assert_eq!(def.kind, *kind);
        assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
    }
}

#[test]
fn the_catalog_reads_back_unchanged() {
    let source: serde_json::Value = serde_json::from_str(include_str!("../src/agents.json")).unwrap();
    assert_eq!(serde_json::to_value(agents()).unwrap(), source);
}

#[test]
fn commands_fill_their_templates() {
    let claude = AgentKind::Claude.def();
    assert_eq!(claude.model_command("opus", None).as_deref(), Some("/model opus"));
    assert_eq!(claude.effort_command("xhigh").as_deref(), Some("/effort xhigh"));
    assert_eq!(claude.goal_command("tests pass").as_deref(), Some("/goal tests pass"));
    assert_eq!(claude.ultracode_command(true), Some("/effort ultracode on"));
    assert_eq!(claude.ultracode_command(false), Some("/effort ultracode off"));
    assert_eq!(claude.set.clear_goal.as_deref(), Some("/goal clear"));
    let agy = AgentKind::Antigravity.def();
    assert_eq!(agy.model_command("gemini-3.8-flash", Some("low")).as_deref(), Some("/model gemini-3.8-flash-low"));
    assert_eq!(agy.model_command("gemini-3.1-pro", Some("medium")).as_deref(), Some("/model gemini-3.1-pro-high"));
    assert_eq!(agy.model_command("claude-sonnet-4-6", Some("low")).as_deref(), Some("/model claude-sonnet-4-6"));
    assert_eq!(agy.ultracode_command(true), None);
    let gemini = AgentKind::Gemini.def();
    assert_eq!(gemini.model_command("gemini-2.5-pro", None), None);
    assert_eq!(gemini.set.compact.as_deref(), Some("/compress"));
}

#[test]
fn agy_model_ids_name_the_effort() {
    assert_eq!(agy_model_id("gemini-3.8-flash", None), "gemini-3.8-flash-high");
    assert_eq!(agy_model_id("gemini-3.8-flash", Some("medium")), "gemini-3.8-flash-medium");
    assert_eq!(agy_model_id("gemini-3.1-pro", Some("low")), "gemini-3.1-pro-low");
    assert_eq!(agy_model_id("gemini-3.1-pro", Some("max")), "gemini-3.1-pro-high");
    assert_eq!(agy_model_id("gemini-3.8-flash-low", Some("high")), "gemini-3.8-flash-low");
    assert_eq!(agy_model_id("gpt-oss-120b-medium", Some("low")), "gpt-oss-120b-medium");
    // only a version of digits and dots names a family (`^gemini-[0-9.]+-(flash|pro)$`)
    assert_eq!(agy_model_id("gemini--pro", None), "gemini--pro");
    assert_eq!(agy_model_id("gemini-3a-flash", None), "gemini-3a-flash");
    assert_eq!(agy_model_id("gemini-3-flash-pro", None), "gemini-3-flash-pro");
    assert_eq!(agy_model_id("gemini-...-pro", None), "gemini-...-pro-high");
}

#[test]
fn agents_are_spotted_by_process_name_or_command_line() {
    assert_eq!(detect_agent("claude", ""), Some(AgentKind::Claude));
    assert_eq!(detect_agent("node", "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"), Some(AgentKind::Claude));
    assert_eq!(detect_agent("node", "/usr/bin/node /home/me/.npm/bin/codex --yolo"), Some(AgentKind::Codex));
    assert_eq!(detect_agent("bun", "bun /x/opencode-ai/bin/opencode"), Some(AgentKind::Opencode));
    assert_eq!(detect_agent("agy", ""), Some(AgentKind::Antigravity));
    assert_eq!(detect_agent("python3", "python3 -m http.server"), None);
    // a shell whose command line mentions an agent isn't one
    assert_eq!(detect_agent("zsh", "zsh -c claude"), None);
    assert_eq!(detect_agent("-bash", "codex"), None);
}

#[test]
fn the_terminal_first_agents_are_spotted_as_their_installs_run_them() {
    // `ps -o comm,args` from each one's own installer or npm package, in a container
    let seen = [
        ("MainThread", "/home/u/.local/bin/cursor-agent --use-system-ca /home/u/.local/share/cursor-agent/versions/2026.10.22-27dcf5f/index.js", AgentKind::Cursor),
        ("MainThread", "/home/u/.local/bin/agent --use-system-ca /home/u/.local/share/cursor-agent/versions/2026.10.22-27dcf5f/index.js", AgentKind::Cursor),
        ("grok", "grok", AgentKind::Grok),
        ("pi", "pi", AgentKind::Pi),
        ("amp", "amp", AgentKind::Amp),
        ("goose", "goose", AgentKind::Goose),
        ("goose", "/home/u/.local/bin/goose session", AgentKind::Goose),
        ("hermes", "/home/u/.hermes/tools/python-3.14.9/bin/python3 -I -c import os, re, sys", AgentKind::Hermes),
        ("kimi", "kimi", AgentKind::Kimi),
        ("kiro-cli", "kiro-cli", AgentKind::Kiro),
        ("node", "/home/u/.local/lib/qwen-code/node/bin/node /home/u/.local/lib/qwen-code/lib/cli-entry.js", AgentKind::Qwen),
        ("node", "node /home/u/.npm-global/bin/qwen", AgentKind::Qwen),
        ("node", "node /home/u/.npm-global/bin/qodercli", AgentKind::Qoder),
        ("codebuddy", "codebuddy", AgentKind::Codebuddy),
        ("node", "node /home/u/.npm-global/bin/codebuddy", AgentKind::Codebuddy),
    ];
    for (comm, args, kind) in seen {
        assert_eq!(detect_agent(comm, args), Some(kind), "{comm} {args}");
    }
    // a file named after an agent isn't one
    assert_eq!(detect_agent("less", "less /tmp/notes/grok"), None);
    assert_eq!(detect_agent("node", "node /srv/app/pipeline.js"), None);
    // what the app reads as chat, and what stays in the terminal
    assert!(AgentKind::Claude.chat() && AgentKind::Gemini.chat());
    assert!(AGENT_KINDS[FIRST_KINDS..].iter().all(|k| !k.chat()));
}

#[test]
fn an_app_before_the_new_agents_knows_only_the_first_six() {
    assert!(AGENT_KINDS[..FIRST_KINDS].iter().all(|k| k.known_to(1)));
    assert!(AGENT_KINDS[FIRST_KINDS..].iter().all(|k| !k.known_to(1) && k.known_to(2)));
}
