//! Windows console Tab events can submit a partial paste. Keep those drafts in Chat
//! and submit the exact text through Codex's existing local queue API.
use inyon_protocol::AgentKind;
use serde_json::json;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;

pub fn required(host_id: &str, pane_id: &str, text: &str) -> bool {
    text.contains('\t')
        && registry().get(host_id).is_some_and(|r| r.borrow().state.os.as_deref() == Some("Windows"))
        && registry().pane_agent(host_id, pane_id).is_some_and(|a| a.kind == AgentKind::Codex)
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
