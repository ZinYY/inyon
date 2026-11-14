//! A correlated refresh after a native conversation operation. Resubscribing alone can
//! return the previous reader's snapshot before the operation has actually finished.
use inyon_protocol::{AgentKind, DraftSource, ScreenState, ServerMsg};
use serde::{Deserialize, Serialize};
use crate::clock::now_ms;
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;
use crate::platform::rt::sleep;
use super::{actions::current_screen, drafts::{get_draft, Draft}, watcher::watchers};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOptions {
    pub agent: Option<AgentKind>,
    pub session: Option<String>,
    pub different_from: Option<String>,
    #[serde(default)] pub stopped: bool,
    #[serde(default)] pub allow_working: bool,
    #[serde(default)] pub allow_prompt: bool,
}

#[derive(Serialize)]
pub struct RefreshedConversation {
    pub reset: ServerMsg,
    pub screen: ScreenState,
    pub draft: Option<Draft>,
}

fn screen_ready(screen: &ScreenState, options: &RefreshOptions) -> bool {
    (options.allow_working || !screen.working)
        && ((options.allow_prompt && (screen.prompt.is_some() || screen.dialog.is_some()))
            || (screen.input_visible == Some(true) && screen.prompt.is_none() && screen.dialog.is_none()))
}

pub async fn refresh(host: &str, pane: &str, options: RefreshOptions) -> HubResult<RefreshedConversation> {
    let deadline = now_ms() + 90_000;
    let mut stable = 0;
    let mut previous = None;
    let mut reloaded = false;
    while now_ms() < deadline {
        registry().refresh_one(host, 6000).await;
        let found = registry().find_pane(host, pane).ok_or_else(|| http(404, "Pane not found"))?;
        let agent = found.pane.agent.as_ref();
        let identity = agent.map(|a| (a.kind, a.pid, a.session_id.clone()));
        let matches = if options.stopped { agent.is_none() } else { agent.is_some_and(|a| {
            options.agent.is_none_or(|k| k == a.kind)
                && options.session.as_ref().is_none_or(|s| a.session_id.as_ref() == Some(s))
                && options.different_from.as_ref().is_none_or(|s| a.session_id.as_ref() != Some(s)
                    && (a.session_id.is_some() || (a.kind == AgentKind::Codex && registry().codex_is_empty(host, pane))))
        }) };
        let w = watchers().get(host, pane);
        w.sync();
        // This also saves Claude's selected rewind branch and restored input before
        // any replacement reader is created. Never replace it mid-picker.
        w.poll_screen().await;
        let screen = if options.stopped { w.screen() } else { current_screen(host, pane).await? };
        if matches && (options.stopped || screen_ready(&screen, &options)) {
            stable = if previous == identity { stable + 1 } else { 1 };
            previous = identity.clone();
            if stable >= 3 {
                let prompt = screen.prompt.is_some() || screen.dialog.is_some();
                if !reloaded && !prompt && agent.and_then(|a| a.transcript_path.as_deref()).is_some_and(|s| !s.is_empty()) {
                    watchers().reload(host, pane)?;
                    reloaded = true;
                }
                let fresh = watchers().get(host, pane);
                fresh.sync();
                let readable = agent.and_then(|a| a.transcript_path.as_deref()).is_none_or(|p| p.is_empty() || p.starts_with("codex-side:")) || fresh.following();
                if let Some(reset) = fresh.snapshot().filter(|_| readable || options.stopped) {
                    let draft = if screen.input_visible == Some(true) && screen.draft_clipped.is_none() {
                        Some(Draft { text: screen.draft.clone().unwrap_or_default(), source: DraftSource::Terminal, at: now_ms() as f64, mirrored: screen.draft.clone() })
                    } else { get_draft(&inyon_protocol::chat_key(host, pane)) };
                    return Ok(RefreshedConversation { reset, screen, draft });
                }
            }
        } else {
            stable = 0;
            previous = identity;
        }
        sleep(500).await;
    }
    Err(http(409, "The conversation has not finished changing. Refresh its status or check the terminal before continuing."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redraw_and_working_frames_do_not_unlock() {
        let mut s: ScreenState = serde_json::from_value(serde_json::json!({"working":false,"width":100})).unwrap();
        let o = RefreshOptions::default();
        assert!(!screen_ready(&s, &o));
        s.input_visible = Some(true);
        assert!(screen_ready(&s, &o));
        s.working = true;
        assert!(!screen_ready(&s, &o));
        assert!(screen_ready(&s, &RefreshOptions { allow_working: true, ..o }));
    }
    #[test]
    fn intermediate_picker_requires_explicit_permission() {
        let s: ScreenState = serde_json::from_value(serde_json::json!({"working":false,"width":100,"dialog":"Rewind"})).unwrap();
        assert!(!screen_ready(&s, &RefreshOptions::default()));
        assert!(screen_ready(&s, &RefreshOptions { allow_prompt: true, ..Default::default() }));
    }
}
