//! The shell the hub sends to machines, byte for byte the Bun hub's: fixtures/expected/scripts.json
//! is what its `scripts()` wrote before it was removed in 0.23, and this builds the same object from
//! the Rust port. A script changed on purpose means changing the file in the same commit.

use std::path::Path;

use inyon_core::agents::{claude_usage, copilot_rpc_sh};
use inyon_core::claude::actions::{remain_on_exit, respawn_shell, resume_command, resume_command_argv};
use inyon_core::claude::{integration, integration_sh};
use inyon_core::features::setup::PATHS;
use inyon_core::hosts::{host, node_update, tidy, via_backend};
use inyon_core::terminal::{mirror, terminal};
use inyon_core::tmux::tmux;
use inyon_protocol::{AgentKind, HostConfig};
use serde_json::{json, Value};

fn ssh(v: Value) -> HostConfig {
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_scripts_and_command_builders_send_the_bytes_the_ts_sends() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/expected/scripts.json");
    let want: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let id = "0f7c9d2e-1111-4222-8333-944455556666";
    let got = json!({
        "UTF8_TMUX": host::UTF8_TMUX,
        "MARKER": integration_sh::MARKER,
        "CURRENT_SH": integration_sh::CURRENT_SH,
        "PANE_SH": integration_sh::PANE_SH,
        "MOVE_OLD_FOLDER_SH": integration_sh::MOVE_OLD_FOLDER_SH,
        "ACCOUNT_SH": integration_sh::ACCOUNT_SH,
        "HOOK_STATE_SH": tmux::HOOK_STATE_SH,
        "PROBE": tmux::PROBE,
        "HOOK_SCRIPT": integration::HOOK_SCRIPT,
        "STATUS_SCRIPT": integration::STATUS_SCRIPT,
        "AGY_STATUS_SCRIPT": integration::AGY_STATUS_SCRIPT,
        "WINDOWS_LAUNCHER": node_update::WINDOWS_LAUNCHER,
        "SELF_UPDATE": node_update::SELF_UPDATE,
        "LINK_UPDATE": node_update::LINK_UPDATE,
        "READ_STATUS": node_update::READ_STATUS,
        "CLAUDE_USAGE": claude_usage::SCRIPT,
        "COPILOT_CATALOG": copilot_rpc_sh::COPILOT_CATALOG_SCRIPT,
        "VIEW_KEYS": terminal::VIEW_KEYS,
        "PATHS": PATHS,
        "commands": {
            "paste": tmux::paste_command("inyon-1", "%1 it's"),
            "typed": match tmux::type_text("%1 it's", "hi", true, "inyon-1") { tmux::TypeText::Keys { command, .. } => command, _ => String::new() },
            "via": via_backend::via_command(&ssh(json!({ "id": "s", "name": "s", "kind": "ssh", "target": "o'brien@db", "port": 2222, "via": "j" })), r#"echo "$HOME" 'x'"#, false),
            "viaTty": via_backend::via_command(&ssh(json!({ "id": "s", "name": "s", "kind": "ssh", "target": "db", "via": "j" })), "tmux attach", true),
            "dropKey": tidy::drop_authorized_key("ssh-ed25519 AAAA it's me"),
            "read": mirror::read_command("%3"),
            "remainOn": remain_on_exit(false, "'%1'", true),
            "remainOffWindows": remain_on_exit(true, "'%1'", false),
            "respawn": respawn_shell(false, "'%1'", Some("/home/o b")),
            "respawnWindows": respawn_shell(true, "'%1'", None),
            "respawnHash": respawn_shell(false, "'%1'", Some("/srv/x#(id)")),
            "view": terminal::view_command("inyon-view-t", "@1", "%1"),
            "resumeClaude": resume_command_argv(AgentKind::Claude, Some(id), &argv(&["node", "/x/claude", "--model", "opus", "--resume", "abc", "hello there", "--add-dir=/a b"])),
            "resumeCodex": resume_command(AgentKind::Codex, Some(id), "/usr/bin/codex resume 0f7c9d2e-1111-4222-8333-944455556666 -m gpt-5 --yolo"),
            "resumeAgy": resume_command_argv(AgentKind::Antigravity, None, &argv(&["agy", "--model", "gemini-3", "-i", "hi", "--continue"])),
        },
    });
    for (k, v) in want.as_object().unwrap() {
        if let Value::Object(cmds) = v {
            for (c, w) in cmds {
                assert_eq!(&got["commands"][c], w, "commands.{c}");
            }
        } else {
            assert_eq!(&got[k], v, "{k}");
        }
    }
    assert_eq!(got, want);
}
