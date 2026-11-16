//! Windows console input can submit literal Tabs and drop non-ASCII characters.
//! Keep those drafts in Chat and send exact text through the existing local API.
use inyon_protocol::AgentKind;
use serde_json::json;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;

fn literal_input(text: &str) -> bool {
    text.contains('\t') || (!text.is_ascii() && !text.trim_start().starts_with(['/', '!']))
}

pub fn required(host_id: &str, pane_id: &str, text: &str) -> bool {
    literal_input(text)
        && registry().get(host_id).is_some_and(|r| r.borrow().state.os.as_deref() == Some("Windows"))
        && registry().pane_agent(host_id, pane_id).is_some_and(|a| a.kind == AgentKind::Codex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_characters_use_direct_input_without_converting_terminal_commands_to_messages() {
        for text in ["coffee—then a walk", "a café ☕", "first\tsecond"] { assert!(literal_input(text)); }
        for text in ["coffee then a walk", "/btw visit a café", "!echo café"] { assert!(!literal_input(text)); }
        assert!(literal_input("/model\tname"), "tabs in commands must still be rejected before typing");
    }
}

pub async fn submit(host: &Host, thread: &str, id: &str, text: &str, steer: bool, turn: Option<&str>) -> HubResult<()> {
    let command = format!("{}; for p in python3 python; do if \"$p\" -c 'import sys; sys.exit(sys.version_info[0] != 3)' </dev/null >/dev/null 2>&1; then exec \"$p\" -c {}; fi; done; exit 1",
        crate::features::setup::PATHS, shq(include_str!("codex_input.py")));
    let input = json!({"thread":thread, "id":id, "text":text, "steer":steer, "turn":turn}).to_string();
    let r = host.exec(&command, ExecOptions::input(input).with_timeout(25000)).await?;
    let reply = serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_default();
    if r.code == 0 && reply["ok"] == true { return Ok(()); }
    Err(http(409, reply["error"].as_str().unwrap_or("Direct Codex input is unavailable. Your message is saved in Chat; terminal input was not used.")))
}
