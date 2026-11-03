//! Port of agents/copilot-controls.ts: changing Copilot's model, permissions or execution mode
//! through its own slash commands, checking the screen after each step.

use super::copilot_models::copilot_models;
use super::copilot_screen::parse_copilot_screen;
use crate::hosts::host::Host;
use crate::hosts::http_error::{HubError, HubResult};
use crate::platform::rt::sleep;
use crate::re;
use crate::tmux::tmux::{capture_panes, send_keys, type_into};

/// What to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotControl {
    Model(String),
    /// "default", "assisted" or "allow-all"
    Permission(String),
    /// "interactive", "plan" or "autopilot"
    Mode(String),
}

struct View {
    lines: Vec<String>,
    screen: inyon_protocol::ScreenState,
}

async fn read(host: &Host, pane: &str) -> HubResult<View> {
    let lines: Vec<String> = capture_panes(host, &[pane.to_owned()])
        .await?
        .shift_remove(pane)
        .unwrap_or_default()
        .iter()
        .map(|l| re!(r"\x1b\[[0-?]*[ -/]*[@-~]", "g").replace_all(l, "").into_owned())
        .collect();
    let screen = parse_copilot_screen(&lines, None);
    Ok(View { lines, screen })
}

fn err(msg: &str) -> HubError {
    HubError::plain(msg)
}

fn execution_mode(lines: &[String]) -> &'static str {
    let footer = lines.iter().rev().find(|l| re!(r"(?:Manual Approval|Assisted Approval|Allow All)").is_match(l)).map_or("", String::as_str);
    if re!(r"(?:^|·) Plan ·").is_match(footer) {
        "plan"
    } else if re!(r"(?:^|·) Autopilot ·").is_match(footer) {
        "autopilot"
    } else {
        "interactive"
    }
}

fn has_draft(v: &View) -> bool {
    v.screen.draft.as_deref().is_some_and(|d| !d.is_empty())
}

/// Whether a fresh read shows the control took effect.
type Matches = Box<dyn Fn(&View) -> bool>;

/// Caller holds the shared pane action lock and pauses draft mirroring before using this.
pub async fn set_copilot_control(host: &Host, pane: &str, control: &CopilotControl) -> HubResult<()> {
    let before = read(host, pane).await?;
    if before.screen.working || before.screen.prompt.is_some() || before.screen.input_visible != Some(true) || has_draft(&before) {
        return Err(err("Copilot must be idle with an empty input before changing settings. Your draft has been preserved."));
    }
    let (command, matches): (String, Matches) = match control {
        CopilotControl::Model(model) => {
            let catalog = copilot_models(host).await;
            if !catalog.models.iter().any(|m| m.id == *model) {
                return Err(err("This model is not available in the signed-in Copilot catalog"));
            }
            let want = model.to_lowercase();
            (format!("/model {model}"), Box::new(move |v: &View| v.screen.model.as_deref().map(str::to_lowercase).as_deref() == Some(want.as_str())))
        }
        CopilotControl::Permission(p) => {
            if !["default", "assisted", "allow-all"].contains(&p.as_str()) {
                return Err(err("Invalid Copilot permission mode"));
            }
            let want = p.clone();
            (format!("/permissions {p}"), Box::new(move |v: &View| v.screen.mode.as_deref() == Some(want.as_str())))
        }
        CopilotControl::Mode(mode) => {
            if !["interactive", "plan", "autopilot"].contains(&mode.as_str()) {
                return Err(err("Invalid Copilot execution mode"));
            }
            if execution_mode(&before.lines) == mode {
                return Ok(());
            }
            for _ in 0..3 {
                send_keys(host, pane, &["BTab"]).await?;
                sleep(350).await;
                let view = read(host, pane).await?;
                if view.screen.prompt.is_some() {
                    return Err(err("Copilot needs confirmation before changing execution mode"));
                }
                if execution_mode(&view.lines) == mode {
                    return Ok(());
                }
            }
            return Err(err("Copilot did not confirm the execution mode"));
        }
    };
    type_into(host, pane, &command, false).await?;
    let mut typed = false;
    for _ in 0..15 {
        sleep(150).await;
        let view = read(host, pane).await?;
        let draft = view.screen.draft.as_deref().unwrap_or("");
        if draft == command || (command == "/model auto" && draft == "/model auto [efficiency|balance|intelligence|<tier>]") || (command == "/plan" && draft == "/plan [prompt]") {
            typed = true;
            break;
        }
        if !draft.is_empty() {
            break;
        }
    }
    if !typed {
        return Err(err("Copilot input changed before the setting was submitted. Check the terminal."));
    }
    send_keys(host, pane, &["Enter"]).await?;
    for _ in 0..20 {
        sleep(150).await;
        let view = read(host, pane).await?;
        if let Some(failure) = view.lines.iter().rev().find(|l| re!(r"[✗×] Error:").is_match(l) && !before.lines.contains(l)) {
            let t = re!(r"^[✗×]\s*").replace(failure.trim(), "");
            return Err(HubError::plain(re!(r"\s*┃$").replace(&t, "").into_owned()));
        }
        if !has_draft(&view) && view.screen.prompt.is_none() && matches(&view) {
            return Ok(());
        }
        if view.screen.prompt.is_some() {
            return Err(err("Copilot needs confirmation in the terminal before this setting can change"));
        }
    }
    Err(err("Copilot did not confirm the setting. Check the terminal before retrying."))
}
