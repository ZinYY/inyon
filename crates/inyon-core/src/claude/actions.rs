//! Port of claude/actions.ts: everything the app does to an agent's pane. Sending a message and
//! following it until the agent has it, answering its questions, switching models and modes
//! through the agent's own menus, starting, restarting, forking and ending sessions.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use inyon_protocol::{agy_model_id, prompt_key, AgentCatalog, AgentKind, AgentState, CatalogSource, ScreenPrompt, ScreenState, SendStatus, AGENT_KINDS, CONFIRM_OPTION};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::delivery::{codex_accepted, has_paste_preview, input_contains};
use super::drafts::clear_input;
use super::driving::start_driving;
use super::keep_default::{keep_claude_defaults, KeepOptions};
use super::screen::{codex_browsing, parse_screen, CodexBrowsing};
use super::send_check;
use super::watcher::watchers;
use crate::agents::antigravity_models::agy_models;
use crate::agents::codex_models::codex_models;
use crate::agents::copilot_controls::{set_copilot_control, CopilotControl};
use crate::agents::opencode::via_open_code_api;
use crate::agents::opencode_api::{opencode_catalog_cached, OpenCodeClient, OpenCodeSelection};
use crate::agents::opencode_controls::{selection_from_footer, selection_from_messages, stored_selection_of};
use crate::agents::opencode_keys::opencode_keys_of;
use crate::agents::opencode_models::{model_from_footer, opencode_cli_models};
use crate::clock::now_ms;
use crate::features::paths::is_task_file;
use crate::features::setup::PATHS;
use crate::hosts::host::{shq, tmux_literal, ExecOptions, Host};
use crate::hosts::http_error::{http, HubError, HubResult};
use crate::hosts::names::{bare_title, original_title, owns_title, pending_name, set_session_name};
use crate::hosts::registry::{registry, width_of, FoundPane, RegistryEvent};
use crate::js::fmt_number;
use crate::js::json::Js;
use crate::js::re::dynamic;
use crate::js::text::{lower, squash, JsStr};
use crate::platform::rt::{set_timeout, sleep, spawn, spawn_task, Deferred};
use crate::re;
use crate::tmux::tmux::{capture_panes, send_keys, type_into};

fn agent_pane(host_id: &str, pane_id: &str) -> HubResult<(Host, FoundPane)> {
    let found = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    Ok((registry().host(host_id)?, found))
}

/// The pane's screen as it is now, read off a fresh capture.
pub async fn current_screen(host_id: &str, pane_id: &str) -> HubResult<ScreenState> {
    let host = registry().host(host_id)?;
    let caps = capture_panes(&host, &[pane_id.to_owned()]).await?;
    let pane = registry().find_pane(host_id, pane_id).map(|f| f.pane);
    let width = pane.as_ref().and_then(width_of);
    let kind = pane.and_then(|p| p.agent).map(|a| a.kind).unwrap_or(AgentKind::Claude);
    Ok(parse_screen(caps.get(pane_id).map(Vec::as_slice).unwrap_or(&[]), width, kind))
}

fn nudge(host_id: &str, pane_id: &str) {
    let w = watchers().peek(host_id, pane_id);
    if let Some(w) = &w {
        w.kick_screen();
    }
    set_timeout(350, move || {
        if let Some(w) = w {
            w.kick_screen();
        }
    });
    registry().refresh(host_id, 250);
}

fn nonempty(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|s| !s.is_empty())
}

fn name_of(kind: AgentKind) -> &'static str {
    &kind.def().name
}

fn keys_of(list: &[String]) -> Vec<&str> {
    list.iter().map(String::as_str).collect()
}

fn repeat(key: &'static str, n: usize) -> Vec<&'static str> {
    vec![key; n]
}

/// agents whose input box Inyon can read back off the screen
const CHECKED: [AgentKind; 5] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity, AgentKind::Copilot, AgentKind::Opencode];

thread_local! {
    static LANES: RefCell<HashMap<String, Deferred<()>>> = RefCell::new(HashMap::new());
    static RENAME_TRIED: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// when each pane's last stop ended (u64::MAX while one runs)
    static STOPPING: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// Lets the next action in the lane go when this one ends, however it ends.
struct LaneTurn {
    key: String,
    done: Deferred<()>,
}

impl Drop for LaneTurn {
    fn drop(&mut self) {
        LANES.with(|l| {
            let mut l = l.borrow_mut();
            if l.get(&self.key).is_some_and(|d| d.same(&self.done)) {
                l.remove(&self.key);
            }
        });
        self.done.resolve(());
    }
}

/// One keyboard per pane: actions that type into it take turns instead of failing each other.
/// The turn is taken when this is called, and the action runs as a task of its own, so it always
/// runs to the end as the TS promise chain does: a caller that stops waiting (an HTTP client that
/// went away) neither lets the next action in early nor leaves this one half typed.
fn in_lane<T, Fut>(host_id: &str, pane_id: &str, f: impl FnOnce() -> Fut + 'static) -> impl Future<Output = HubResult<T>> + 'static
where
    T: Clone + 'static,
    Fut: Future<Output = HubResult<T>> + 'static,
{
    let key = crate::js::json::stringify(&json!([host_id, pane_id]));
    let done = Deferred::new();
    let prev = LANES.with(|l| l.borrow_mut().insert(key.clone(), done.clone()));
    let result = spawn_task(async move {
        let _turn = LaneTurn { key, done };
        if let Some(p) = prev {
            p.wait().await;
        }
        f().await
    });
    result.wait()
}

/// Sends a message and follows it until the agent has it. `id` names the send for every device
/// (the app passes its own, so a retry is the same send); its state is in the chat info's `sends`.
/// `from_composer`: the text came out of the app's message box, so the saved draft goes with it.
/// `setting`: a change made with the app's controls, which no device shows as a message.
/// `steer`: while Codex works, hand it the message in the middle of its turn rather than queueing it.
///
/// The send runs as a task of its own, so it is typed, followed and settled to the end even when
/// the caller stops waiting for it.
pub async fn send_message(host_id: &str, pane_id: &str, text: &str, from_composer: bool, id: Option<&str>, setting: bool, steer: bool) -> HubResult<()> {
    let (host_id, pane_id, text) = (host_id.to_owned(), pane_id.to_owned(), text.to_owned());
    let id = id.map(str::to_owned).unwrap_or_else(|| format!("hub-{}", crate::ids::random_uuid()));
    spawn_task(async move { send_and_follow(&host_id, &pane_id, &text, from_composer, id, setting, steer).await }).wait().await
}

async fn send_and_follow(host_id: &str, pane_id: &str, text: &str, from_composer: bool, id: String, setting: bool, steer: bool) -> HubResult<()> {
    // refused before it's followed: an empty send that was would be matched to whatever the agent
    // writes next and show as delivered
    check_send(host_id, pane_id, text)?;
    let w = watchers().get(host_id, pane_id);
    w.track_send(&id, text, setting);
    let r = async {
        let (h, p, t) = (host_id.to_owned(), pane_id.to_owned(), text.to_owned());
        in_lane(host_id, pane_id, move || async move { send_message_to_pane(&h, &p, &t, from_composer, steer).await }).await?;
        confirm_delivery(host_id, pane_id, &id, text).await
    }
    .await;
    if let Err(e) = &r {
        w.settle_send(&id, Some(SendStatus::Failed), Some(e.message.clone()));
        keep_failure(host_id, pane_id, &e.message, text).await;
    }
    w.idle_later();
    r
}

/// The pane's screen as plain lines.
async fn screen_lines(host: &Host, pane_id: &str) -> HubResult<Vec<String>> {
    Ok(capture_panes(host, &[pane_id.to_owned()]).await?.shift_remove(pane_id).unwrap_or_default())
}

/// Writes a failed send down with the screen it failed on (see send_check).
async fn keep_failure(host_id: &str, pane_id: &str, error: &str, text: &str) {
    let agent = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent).map(|a| a.kind.as_str());
    let screen = match registry().host(host_id) {
        Ok(host) => screen_lines(&host, pane_id).await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    send_check::record_failure(host_id, pane_id, agent, error, text, &screen);
}

/// Keys accepted on screen don't prove the agent took the message: a dialog can swallow them, or
/// the box can be cleared under us. Agents write every message down at once (queued while they
/// work, or as a new turn), and the ledger matches it, so wait for that. A new conversation's
/// transcript only appears with its first message, so not following yet is part of the wait.
async fn confirm_delivery(host_id: &str, pane_id: &str, id: &str, text: &str) -> HubResult<()> {
    let w = watchers().get(host_id, pane_id);
    // a slash command often opens a menu or changes a setting without writing anything down
    if text.jtrim_start().starts_with('/') {
        w.settle_send(id, Some(SendStatus::Delivered), None);
        return Ok(());
    }
    w.settle_send(id, Some(SendStatus::Sent), None);
    for _ in 0..48 {
        if matches!(w.delivery(id).map(|d| d.state), Some(SendStatus::Queued | SendStatus::Delivered)) {
            return Ok(());
        }
        sleep(250).await;
    }
    let agent = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent);
    if agent.is_some_and(|a| a.kind == AgentKind::Claude) && w.following() {
        return Err(http(
            409,
            "Claude didn't record the message, so it may not have reached it (a dialog on screen can take the keys). Open the terminal to check before retrying.",
        ));
    }
    // typed and accepted on screen, but not in a record we can read yet: the ledger keeps looking
    Ok(())
}

/// Claude Code's panel for one background shell or monitor (opened from its "↓ to manage" list,
/// easily by accident) covers the input box and takes every key, so a message typed there never
/// arrives.
pub fn claude_details_panel(lines: &[String]) -> bool {
    let text = re!(r"\x1b\[[0-9;]*m", "g").replace_all(&lines.join("\n"), "").into_owned();
    let tail: Vec<&str> = text.lines().rev().take(60).collect();
    tail.iter().any(|l| re!(r"^\s*(Shell|Monitor) details\s*$").is_match(l)) && !tail.iter().any(|l| re!(r"^\s*❯").is_match(l))
}

/// Closes that panel with Esc, which only dismisses it, so the message goes to the input box.
async fn close_claude_details(host: &Host, pane_id: &str) -> HubResult<()> {
    for _ in 0..3 {
        if !claude_details_panel(&screen_lines(host, pane_id).await?) {
            break;
        }
        send_keys(host, pane_id, &["Escape"]).await?;
        sleep(300).await;
    }
    Ok(())
}

/// Claude's preview of a paste, and nothing else: text left in front of it would be sent too.
fn only_paste_preview(squashed: &str) -> bool {
    re!(r"^(\[Pastedtext#\d+(\+\d+lines?)?\])+$").is_match(squashed)
}

/// Clears the input box and waits until it reads empty. Pasting next to leftovers of an older
/// draft would send both, with the start of the message twice.
async fn empty_input(host_id: &str, pane_id: &str, mut draft: String) -> HubResult<()> {
    for _round in 0..3 {
        clear_input(host_id, pane_id, &draft).await?;
        for i in 0..6 {
            sleep(if i < 2 { 60 } else { 120 }).await;
            match current_screen(host_id, pane_id).await?.draft {
                Some(left) if !left.is_blank() => draft = left,
                _ => return Ok(()),
            }
        }
    }
    Err(http(409, "Couldn't clear what was already in the agent's input box, so nothing was sent. Open the terminal to check."))
}

/// What makes a message unsendable before anything is typed (send_message_to_pane checks the same).
fn check_send(host_id: &str, pane_id: &str, text: &str) -> HubResult<()> {
    let (_, found) = agent_pane(host_id, pane_id)?;
    if found.pane.agent.is_none() {
        return Err(http(409, "No agent is running in this pane"));
    }
    if text.is_blank() {
        return Err(http(400, "Message is empty"));
    }
    Ok(())
}

/// Types a message into the agent's input box and submits it. For Claude we confirm on screen that
/// the text arrived before pressing Enter (a fast Enter can be swallowed while Claude is still
/// taking in the typing), then confirm the box emptied, retrying Enter if it didn't.
///
/// While Codex works, Enter steers: the message goes into the turn under way. Tab queues it for
/// after the turn (the TUI's own hint, "tab to queue message"), which is what a message sent
/// without `steer` does.
pub async fn send_message_to_pane(host_id: &str, pane_id: &str, text: &str, from_composer: bool, steer: bool) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let Some(agent) = found.pane.agent.clone() else { return Err(http(409, "No agent is running in this pane")) };
    if text.is_blank() {
        return Err(http(400, "Message is empty"));
    }
    let started_at = if from_composer { now_ms() } else { 0 };
    if has_paste_preview(text) && re!(r"^\[Pasted[^\n]+\]$", "i").is_match(text.jtrim()) {
        return Err(http(409, "This saved message contains only a terminal paste preview. Paste the original text again; the preview does not contain the message."));
    }
    let kind = agent.kind;
    if kind == AgentKind::Opencode {
        // model, variant and agent go through OpenCode's own keys in both modes, so its footer and the
        // app always agree (see open_code_model)
        if let Some(c) = re!(r"^/(model|effort|variant|agent)\s+(\S+)$").captures(text.jtrim()) {
            let arg = c[2].to_owned();
            match &c[1] {
                "model" => return open_code_model(host_id, pane_id, &arg).await,
                "effort" | "variant" => return open_code_effort(host_id, pane_id, &arg).await,
                _ => return open_code_agent(host_id, pane_id, &arg).await,
            }
        }
        if let Some(c) = re!(r"^/(\S+)\s*$").captures(text.jtrim()) {
            let command = &c[1];
            if OPENCODE_TERMINAL.contains(&command) {
                return Err(http(
                    400,
                    if command == "models" || command == "model" {
                        "Pick the model from the model menu under the message box.".to_owned()
                    } else {
                        format!("/{command} opens a menu in OpenCode's terminal. Open the terminal to use it.")
                    },
                ));
            }
        }
    }
    if kind == AgentKind::Opencode && via_open_code_api(agent.transcript_path.as_deref()) {
        let uri = open_code_session_uri(host_id, pane_id).await?;
        let client = OpenCodeClient::new(&host, &uri, Some(&found.pane.cwd))?;
        let selection = open_code_api_selection(host_id, pane_id, &client).await?;
        if let Some(cmd) = text.jtrim_start().strip_prefix('!') {
            client.shell(cmd.jtrim(), selection.agent.as_deref().unwrap_or("build"), selection.model.as_deref()).await?;
        } else if text.starts_with('/') {
            open_code_api_command(host_id, pane_id, &client, text, &selection).await?;
        } else {
            client.prompt(text, &selection, &[]).await?;
        }
        if let Some(w) = watchers().peek(host_id, pane_id) {
            w.draft_sent(Some(text), started_at);
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    if let Some(w) = watchers().peek(host_id, pane_id) {
        w.begin_send().await;
    }
    if kind == AgentKind::Copilot {
        let model = re!(r"^/model\s+(\S+)$").captures(text.jtrim()).map(|c| c[1].to_owned());
        let permission = re!(r"^/permissions\s+(default|allow-all)$").captures(text.jtrim()).map(|c| c[1].to_owned());
        let control = match (model, permission) {
            (Some(m), _) => Some(CopilotControl::Model(m)),
            (None, Some(p)) => Some(CopilotControl::Permission(p)),
            _ => None,
        };
        if let Some(control) = control {
            set_copilot_control(&host, pane_id, &control).await?;
            nudge(host_id, pane_id);
            return Ok(());
        }
    }
    // Codex takes no arguments to /model; Inyon's "/model id [effort]" and "/effort level" drive its picker
    if kind == AgentKind::Codex {
        let t = text.jtrim();
        let m = re!(r"^/model\s+(\S+)(?:\s+(\S+))?$").captures(t).or_else(|| re!(r"^/effort\s+()(\S+)$").captures(t));
        if let Some(m) = m {
            let model = m.get(1).map(|x| x.as_str().to_owned()).filter(|x| !x.is_empty());
            let effort = m.get(2).map(|x| x.as_str().to_owned());
            return codex_model(host_id, pane_id, model.as_deref(), effort.as_deref()).await;
        }
    }
    // Claude Code saves a /model or /effort pick as the default for new sessions; from Inyon it's for this one
    if kind == AgentKind::Claude && re!(r"^/(model|effort)(\s|$)").is_match(text.jtrim()) {
        keep_claude_defaults(&host, KeepOptions::default()).await;
    }
    if kind == AgentKind::Claude {
        close_claude_details(&host, pane_id).await?;
    }
    let screen = current_screen(host_id, pane_id).await?;
    if screen.prompt.is_some() {
        return Err(http(409, "The agent is waiting for an answer to a question first"));
    }
    let is_codex = kind == AgentKind::Codex;
    let is_agy = kind == AgentKind::Antigravity;
    let is_open_code = kind == AgentKind::Opencode;
    let checked_input = is_codex || is_agy || is_open_code || kind == AgentKind::Copilot;
    let visible = screen.input_visible == Some(true);
    if is_open_code && !visible {
        return Err(http(409, "OpenCode input is not visible. Open the terminal to check before sending."));
    }
    let setting = if is_agy { re!(r"^/(model|effort)\s+(\S+)$").captures(text.jtrim()).map(|c| (c[1].to_owned(), c[2].to_owned())) } else { None };
    let setting_draft = if setting.is_some() { screen.draft.clone().filter(|d| !d.is_empty() && squash(d) != squash(text)) } else { None };
    if is_agy && !visible {
        return Err(http(409, "Antigravity input is not visible. Open the terminal to check before sending."));
    }
    if let Some((what, arg)) = &setting {
        if screen.working {
            return Err(http(409, "Change Antigravity settings when it finishes working"));
        }
        let catalog = agy_models(&host).await;
        if catalog.source == CatalogSource::Cli {
            let offered = if what == "model" {
                catalog.models.iter().any(|m| m.id == *arg || m.variants.as_ref().is_some_and(|v| v.values().any(|x| x == arg)))
            } else {
                let on_screen = lower(screen.model.as_deref().unwrap_or(""));
                catalog.models.iter().find(|m| lower(&m.label) == on_screen).and_then(|m| m.efforts.as_ref()).is_some_and(|e| e.contains(arg))
            };
            if !offered {
                return Err(http(400, "That model or effort is not offered by this Antigravity session"));
            }
        }
    }
    let release = match (&setting_draft, watchers().peek(host_id, pane_id)) {
        (Some(d), Some(w)) => Some(w.hold_draft(Some(d)).await),
        _ => None,
    };
    let held = release.is_some();
    let typed = Typing { host_id, pane_id, host: &host, text, kind, checked_input, steer, started_at, held, setting_draft: setting_draft.as_deref() };
    let r = typed.run(&screen).await;
    if let Some(release) = release {
        release().await;
    }
    r
}

/// The part of a send that types and submits, between holding a draft and putting it back.
struct Typing<'a> {
    host_id: &'a str,
    pane_id: &'a str,
    host: &'a Host,
    text: &'a str,
    kind: AgentKind,
    checked_input: bool,
    steer: bool,
    started_at: u64,
    held: bool,
    setting_draft: Option<&'a str>,
}

impl Typing<'_> {
    fn arrived(&self, d: Option<&str>, typed: bool) -> bool {
        if self.checked_input {
            return input_contains(d, self.text, typed);
        }
        let now = squash(d.unwrap_or(""));
        // the whole message, Claude's paste preview of it, or the end of a long one whose start has
        // scrolled out of the box. Never "most of it": a draft mirrored from the app before the last
        // edits is a shorter prefix, and pressing Enter on it sends the old text.
        input_contains(d, self.text, typed) || (typed && only_paste_preview(&now)) || (typed && now.len16() >= 200 && squash(self.text).ends_with(&now))
    }

    async fn run(&self, screen: &ScreenState) -> HubResult<()> {
        let (host_id, pane_id, host, text) = (self.host_id, self.pane_id, self.host, self.text);
        let is_codex = self.kind == AgentKind::Codex;
        let is_open_code = self.kind == AgentKind::Opencode;
        if is_codex && !nonempty(&screen.model) {
            return Err(http(409, "Codex input is not visible. Open the terminal to check before sending."));
        }
        // Claude, Codex and agy show their input box, so the text can be checked into place before Enter.
        // Any other agent still gets no Enter until the text shows on its screen
        if !CHECKED.contains(&self.kind) {
            let before = screen_lines(host, pane_id).await?;
            type_into(host, pane_id, text, false).await?;
            let mut shown = false;
            for i in 0..20 {
                sleep(if i < 3 { 80 } else { 150 }).await;
                if send_check::arrived(&before, &screen_lines(host, pane_id).await?, text) {
                    shown = true;
                    break;
                }
            }
            if !shown {
                let name = name_of(self.kind);
                return Err(http(409, format!("The message didn't show in {name}'s input box, so Enter wasn't pressed. Your message is available to copy or retry.")));
            }
            sleep(150).await;
            send_keys(host, pane_id, &["Enter"]).await?;
            if let Some(w) = watchers().peek(host_id, pane_id) {
                w.draft_sent(None, 0);
            }
            nudge(host_id, pane_id);
            return Ok(());
        }
        // set once this send has emptied the box and typed the message: only then is a paste preview
        // known to be this message rather than a shorter draft the mirror put there earlier
        let mut typed = false;
        // the draft may already be in the box (mirrored from the app); otherwise replace whatever is there
        if !input_contains(screen.draft.as_deref(), text, false) {
            // a second go when the first doesn't show up: a mirrored draft still being typed can leave
            // the box with something else in it, which is cleared first. Only then: an empty box or
            // one that can't be read may just be slow to show the text, and typing again would double it.
            let mut draft = screen.draft.clone();
            let mut readable = screen.input_visible == Some(true) || !self.checked_input;
            let mut round = 0;
            while round < 2 && !self.arrived(draft.as_deref(), typed) && (round == 0 || (readable && draft.as_deref().is_some_and(|d| !d.is_empty()))) {
                if let Some(d) = draft.clone().filter(|d| !d.is_empty()) {
                    empty_input(host_id, pane_id, d).await?;
                }
                type_into(host, pane_id, text, false).await?;
                typed = true;
                for i in 0..15 {
                    sleep(if i < 3 { 80 } else { 150 }).await;
                    let now = current_screen(host_id, pane_id).await?;
                    draft = now.draft;
                    readable = now.input_visible == Some(true);
                    if self.arrived(draft.as_deref(), typed) {
                        break;
                    }
                }
                round += 1;
            }
        }
        let name = name_of(self.kind);
        if self.checked_input && !self.arrived(current_screen(host_id, pane_id).await?.draft.as_deref(), typed) {
            return Err(http(409, format!("Could not confirm the message reached {name}. Your message is available to copy or retry.")));
        }
        let slash = text.jtrim_start().starts_with('/');
        for _attempt in 0..3 {
            let before_submit = if self.checked_input { Some(current_screen(host_id, pane_id).await?) } else { None };
            if let Some(b) = &before_submit {
                if b.prompt.is_some() || !(nonempty(&b.model) || b.input_visible == Some(true)) || !self.arrived(b.draft.as_deref(), typed) {
                    return Err(http(409, format!("{name} input changed before sending. Open the terminal to check before retrying.")));
                }
            }
            let working = before_submit.as_ref().is_some_and(|b| b.working);
            let key = if is_codex && working && !self.steer && !text.jtrim_start().starts_with('!') { "Tab" } else { "Enter" };
            send_keys(host, pane_id, &[key]).await?;
            for _ in 0..8 {
                sleep(140).await;
                let s = current_screen(host_id, pane_id).await?;
                // a command that opens the agent's own menu (e.g. a bare /model) has arrived once the menu shows
                let opens_menu = slash
                    && (s.prompt.is_some()
                        || (s.input_visible == Some(false) && !nonempty(&s.draft))
                        // OpenCode's /undo puts the undone message back in the box
                        || (is_open_code && s.input_visible == Some(true) && squash(s.draft.as_deref().unwrap_or("")) != squash(text)));
                let accepted = if self.checked_input { codex_accepted(&s, text) } else { s.prompt.is_some() || !self.arrived(s.draft.as_deref(), typed) };
                if opens_menu || accepted {
                    if !self.held {
                        if let Some(w) = watchers().peek(host_id, pane_id) {
                            w.draft_sent(Some(text), self.started_at);
                        }
                    }
                    if is_open_code && slash {
                        close_open_code_menu(host_id, pane_id).await?;
                    }
                    if let (Some(d), false) = (self.setting_draft, self.held) {
                        type_into(host, pane_id, d, false).await?;
                        if let Some(w) = watchers().peek(host_id, pane_id) {
                            w.draft_from_app(d, None);
                        }
                    }
                    nudge(host_id, pane_id);
                    return Ok(());
                }
            }
        }
        nudge(host_id, pane_id);
        Err(http(409, format!("The text is in {name}'s input box but Enter was not accepted. Open the terminal to check.")))
    }
}

/// Answers a dialog that asks for typed input (e.g. the /login code).
pub async fn input_answer(host_id: &str, pane_id: &str, text: &str) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let s = current_screen(host_id, pane_id).await?;
    if s.prompt.and_then(|p| p.input).is_none() {
        return Err(http(409, "That prompt is no longer on screen"));
    }
    type_into(&host, pane_id, text, false).await?;
    sleep(250).await;
    send_keys(&host, pane_id, &["Enter"]).await?;
    nudge(host_id, pane_id);
    Ok(())
}

/// What stopping a background task did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopTask {
    pub stopped: bool,
    pub detail: String,
}

/// The script that stops whatever writes to a task's output file, with its children.
pub fn stop_task_script(output_file: &str) -> String {
    format!(
        "\nF={}\n{}",
        shq(output_file),
        r#"pids=""
for d in /proc/[0-9]*; do
  for fd in 1 2; do
    [ "$(readlink "$d/fd/$fd" 2>/dev/null)" = "$F" ] && pids="$pids ${d#/proc/}" && break
  done
done
[ -z "$pids" ] && { echo none; exit 0; }
tree() { echo "$1"; for c in $(pgrep -P "$1" 2>/dev/null); do tree "$c"; done; }
all=""; for p in $pids; do all="$all $(tree "$p")"; done
kill -TERM $all 2>/dev/null; sleep 1.5; kill -KILL $all 2>/dev/null
echo "stopped $all""#
    )
}

/// Stops a background task (shell, monitor) for real: finds the processes writing to its output
/// file and terminates them with their children. The agent itself is never touched.
pub async fn stop_task(host_id: &str, output_file: Option<&str>, id: Option<&str>) -> HubResult<StopTask> {
    let host = registry().host(host_id)?;
    let mut file = output_file.filter(|f| !f.is_empty()).map(str::to_owned);
    if file.is_none() {
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            file = find_task_output(host_id, id).await?;
        }
    }
    let home = registry().get(host_id).and_then(|r| r.borrow().probe.as_ref().map(|p| p.borrow().home.clone()));
    let Some(file) = file.filter(|f| is_task_file(f, home.as_deref())) else { return Err(http(404, "Could not find that task on the machine")) };
    let r = host.exec(&stop_task_script(&file), ExecOptions::timeout(15000)).await?;
    let out = r.stdout.jtrim().to_owned();
    registry().refresh(host_id, 300);
    Ok(StopTask { stopped: out.starts_with("stopped"), detail: out })
}

/// Picks an option in the prompt on screen, optionally typing free text after it. `question` is the
/// prompt key of the question the answer was given for; another question on screen now is refused.
pub async fn answer(host_id: &str, pane_id: &str, option: u32, text: Option<&str>, question: Option<&str>) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    if let Some(w) = watchers().peek(host_id, pane_id) {
        if w.answer_api(option, text).await? {
            nudge(host_id, pane_id);
            return Ok(());
        }
    }
    let screen = current_screen(host_id, pane_id).await?;
    let n = u64::from(option);
    let confirming = option == CONFIRM_OPTION && screen.prompt.as_ref().is_some_and(|p| nonempty(&p.confirm));
    let Some(p) = screen.prompt.filter(|p| confirming || p.options.iter().any(|o| o.n == n)) else {
        return Err(http(409, "That question is no longer on screen"));
    };
    let key = prompt_key(&p.body, &p.options);
    if question.is_some_and(|q| !q.is_empty() && key != q) {
        return Err(http(409, "That question was answered and another one is waiting. Check it before answering."));
    }
    let kind = found.pane.agent.as_ref().map(|a| a.kind);
    let is_open_code = kind == Some(AgentKind::Opencode);
    // a permission or a question in the middle of a turn: answered (other than No), Claude carries on
    let label = p.options.iter().find(|o| o.n == n).map(|o| o.label.as_str()).unwrap_or("");
    let carries_on = kind == Some(AgentKind::Claude) && mid_turn(&p) && !re!(r"^no\b", "i").is_match(label);
    if confirming && is_open_code {
        // Tab moves on to the next question or to the Confirm step, where Enter sends every answer
        if p.options.is_empty() {
            send_keys(&host, pane_id, &["Enter"]).await?;
        } else {
            send_keys(&host, pane_id, &["Tab"]).await?;
            let next = wait_screen(host_id, pane_id, |s| s.prompt.as_ref().is_none_or(|sp| prompt_key(&sp.body, &sp.options) != key), 1500).await?;
            if next.prompt.is_some_and(|np| np.confirm.as_deref() == Some("Submit") && np.options.is_empty()) {
                send_keys(&host, pane_id, &["Enter"]).await?;
            }
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    if p.multi && is_open_code {
        // a number toggles its checkbox; the typed answer goes in after its own
        send_keys(&host, pane_id, &[&option.to_string()]).await?;
        if let Some(t) = text.filter(|t| p.free_text_option == Some(n) && !t.is_blank()) {
            sleep(250).await;
            type_into(&host, pane_id, t, true).await?;
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    let navigate = p.navigate == Some(true);
    if navigate && is_open_code {
        // "Allow once  Allow always  Reject" in a row, the highlight shown only by colour: go to the
        // first button, then right
        let mut keys = repeat("Left", p.options.len());
        keys.extend(repeat("Right", (option as usize).saturating_sub(1)));
        keys.push("Enter");
        send_keys(&host, pane_id, &keys).await?;
        // "Allow always" asks to confirm; picking it here was the confirmation
        if (option as usize).checked_sub(1).and_then(|i| p.options.get(i)).is_some_and(|o| o.label == "Allow always") {
            let next = wait_screen(host_id, pane_id, |s| s.prompt.as_ref().is_some_and(|sp| sp.options.iter().any(|o| o.label == "Confirm")), 1500).await?;
            if next.prompt.as_ref().and_then(|np| np.options.first()).is_some_and(|o| o.label == "Confirm") {
                send_keys(&host, pane_id, &["Left", "Left", "Enter"]).await?;
            }
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    if navigate {
        // no numbers on screen: move the cursor from the highlighted item to the target, then Enter
        let from = p.options.iter().find(|o| o.selected).map(|o| o.n as i64).unwrap_or(1);
        let delta = n as i64 - from;
        let mut keys = repeat(if delta > 0 { "Down" } else { "Up" }, delta.unsigned_abs() as usize);
        keys.push("Enter");
        send_keys(&host, pane_id, &keys).await?;
        if carries_on {
            registry().turn_goes_on(host_id, pane_id);
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    send_keys(&host, pane_id, &[&option.to_string()]).await?;
    if let Some(t) = text.filter(|t| !t.is_blank()) {
        sleep(250).await;
        type_into(&host, pane_id, t, true).await?;
    }
    if carries_on {
        registry().turn_goes_on(host_id, pane_id);
    }
    nudge(host_id, pane_id);
    Ok(())
}

/// Claude's tool permissions ("Do you want to proceed?", "Tab to amend") and its questions come mid-turn; menus don't.
fn mid_turn(p: &ScreenPrompt) -> bool {
    p.tabs.as_ref().is_some_and(|t| !t.is_empty())
        || re!(r"Tab to amend", "i").is_match(p.footer.as_deref().unwrap_or(""))
        || p.body.iter().any(|l| re!(r"^Do you want to ").is_match(l.jtrim()))
}

pub async fn keys(host_id: &str, pane_id: &str, list: &[String]) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    send_keys(&host, pane_id, &keys_of(list)).await?;
    // Esc cancels the question and the turn with it
    if list.iter().any(|k| k == "Escape") {
        registry().turn_ended(host_id, pane_id);
    }
    nudge(host_id, pane_id);
    Ok(())
}

/// OpenCode 1.19 commands that open a menu or a view in its terminal instead of acting on the conversation
const OPENCODE_TERMINAL: [&str; 20] = [
    "model", "models", "agents", "variants", "sessions", "themes", "connect", "mcps", "skills", "status", "help", "editor", "diff", "timeline", "debug", "move", "update",
    "export", "copy", "exit",
];

/// The models and agents OpenCode offers this pane: its own server's list when it has one, else the CLI's.
async fn open_code_catalog(host_id: &str, pane_id: &str) -> HubResult<AgentCatalog> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let uri = found.pane.agent.and_then(|a| a.transcript_path);
    if via_open_code_api(uri.as_deref()) {
        if let Ok(c) = opencode_catalog_cached(&host, uri.as_deref().unwrap_or(""), Some(&found.pane.cwd)).await {
            return Ok(c);
        }
    }
    Ok(opencode_cli_models(&host).wait().await)
}

/// The session the pane's terminal shows, with its server. A terminal on its home screen has none
/// yet: one is made and shown there, reusing an empty one OpenCode already made for the folder, so
/// no stray "New session" piles up in its list.
async fn open_code_session_uri(host_id: &str, pane_id: &str) -> HubResult<String> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let uri = found.pane.agent.as_ref().and_then(|a| a.transcript_path.clone()).unwrap_or_default();
    let client = OpenCodeClient::new(&host, &uri, Some(&found.pane.cwd))?;
    if !client.session.is_empty() {
        return Ok(uri);
    }
    let list = client.request("/session", "GET", None, None).await.ok().flatten();
    let now = now_ms() as f64;
    let mut empty: Option<(f64, Value)> = None;
    for s in list.as_ref().and_then(Value::as_array).into_iter().flatten() {
        let s = Some(s);
        let updated = match s.at("time").at("updated") {
            None | Some(Value::Null) => 0.0,
            Some(v) => v.as_f64().unwrap_or(f64::NAN),
        };
        let fits = s.at("id").str().is_some()
            && !s.at("parentID").truthy()
            && !s.at("time").at("archived").truthy()
            && re!(r"^New session - ").is_match(s.at("title").str().unwrap_or(""))
            && now - updated < 3_600_000.0;
        if fits && empty.as_ref().is_none_or(|(at, _)| updated > *at) {
            empty = Some((updated, s.cloned().unwrap_or_default()));
        }
    }
    let created = match empty {
        Some((_, s)) => Some(s),
        None => client.request("/session", "POST", Some(json!({})), None).await?,
    };
    let Some(id) = created.as_ref().at("id").filter(|v| Some(*v).truthy()).map(crate::js::json::value_string) else {
        return Err(http(502, "OpenCode did not create a session"));
    };
    client.select_session(&id).await?;
    let with_session = format!("{uri}{id}");
    registry().update_pane(host_id, pane_id, |p| {
        if let Some(a) = &mut p.agent {
            a.transcript_path = Some(with_session.clone());
            a.session_id = Some(id.clone());
        }
    });
    Ok(with_session)
}

/// What a message sent through the API runs with: whatever the terminal's footer shows (kept by
/// the pane while a menu hides the footer), else what the session last ran with, so the reply
/// matches what the terminal says it will use.
async fn open_code_api_selection(host_id: &str, pane_id: &str, client: &OpenCodeClient) -> HubResult<OpenCodeSelection> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let agent = found.pane.agent.clone().ok_or_else(|| HubError::plain("No agent is running in this pane"))?;
    let screen = current_screen(host_id, pane_id).await?;
    let catalog = opencode_catalog_cached(&host, agent.transcript_path.as_deref().unwrap_or(""), Some(&found.pane.cwd)).await.ok();
    let footer = selection_from_footer(screen.model.as_deref(), screen.effort.as_deref(), screen.mode.as_deref(), catalog.as_ref().map(|c| c.models.as_slice()).unwrap_or(&[]));
    if let Some(f) = footer.filter(|f| nonempty(&f.model)) {
        return Ok(f);
    }
    if let Some(status) = agent.status.as_ref().filter(|s| nonempty(&s.model_id)) {
        return Ok(OpenCodeSelection { model: status.model_id.clone(), variant: status.effort.clone(), agent: agent.mode.clone() });
    }
    let stored = if client.session.is_empty() { OpenCodeSelection::default() } else { stored_selection_of(client).await };
    if nonempty(&stored.model) {
        return Ok(stored);
    }
    let messages = if client.session.is_empty() { None } else { client.messages().await.ok().flatten() };
    Ok(selection_from_messages(messages.as_ref().and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])))
}

/// A slash command for a session with its own server: what the server does itself, else refused.
async fn open_code_api_command(host_id: &str, pane_id: &str, client: &OpenCodeClient, text: &str, selection: &OpenCodeSelection) -> HubResult<()> {
    let caps = re!(r"^/(\S+)(?:\s+([\s\S]*))?$").captures(text);
    let command = caps.as_ref().and_then(|c| c.get(1)).map(|m| m.as_str().to_owned());
    let args = caps.as_ref().and_then(|c| c.get(2)).map(|m| m.as_str().to_owned()).unwrap_or_default();
    match command.as_deref() {
        Some("compact" | "summarize") => {
            let model = match &selection.model {
                Some(m) => Some(m.clone()),
                None => stored_selection_of(client).await.model,
            };
            let Some(model) = model.filter(|m| !m.is_empty()) else {
                return Err(http(409, "Send a message first, so OpenCode knows which model to compact with"));
            };
            client.summarize(&model).await?;
            return Ok(());
        }
        Some("undo") => {
            // back to before the last message still in the conversation (an earlier /undo hides the ones after its mark)
            let (messages, info) = futures_util::future::join(client.messages(), client.info()).await;
            let messages = messages?;
            let info = info.ok().flatten();
            let cutoff = info.as_ref().at("revert").at("messageID").cloned();
            let cutoff = cutoff.filter(|c| Some(c).truthy()).map(|c| crate::js::json::value_string(&c));
            let users: Vec<&Value> = messages
                .as_ref()
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|m| Some(m).at("info").or(Some(m)).unwrap_or(m))
                .filter(|m| Some(*m).at("role").str() == Some("user") && cutoff.as_deref().is_none_or(|c| crate::js::text::utf16_lt(&Some(*m).at("id").to_js_string(), c)))
                .collect();
            let Some(last) = users.last() else { return Err(http(409, "Nothing to undo yet")) };
            client.revert(&Some(*last).at("id").to_js_string()).await?;
            return Ok(());
        }
        Some("redo") => {
            client.unrevert().await?;
            return Ok(());
        }
        Some("share") => {
            client.share().await?;
            return Ok(());
        }
        Some("unshare") => {
            client.unshare().await?;
            return Ok(());
        }
        Some("new") => {
            let created = client.request("/session", "POST", Some(json!({})), None).await?;
            let Some(id) = created.as_ref().at("id").filter(|v| Some(*v).truthy()).map(crate::js::json::value_string) else {
                return Err(http(502, "OpenCode did not create a session"));
            };
            client.select_session(&id).await?;
            registry().refresh(host_id, 300);
            return Ok(());
        }
        _ => {}
    }
    let _ = pane_id;
    let commands = client.request("/command", "GET", None, None).await?;
    let name = command.as_deref().unwrap_or("undefined");
    let offered = commands.as_ref().and_then(Value::as_array).is_some_and(|list| {
        list.iter().any(|c| match &command {
            Some(cmd) => Some(c).at("name").str() == Some(cmd.as_str()),
            None => Some(c).at("name").is_none(),
        })
    });
    if !offered {
        return Err(http(400, format!("/{name} is a control of OpenCode's terminal. Open the terminal to use it.")));
    }
    client.command(name, &args, selection).await?;
    Ok(())
}

/// OpenCode's box must be showing (no question, no menu) before its keys are pressed.
async fn open_code_ready(host_id: &str, pane_id: &str) -> HubResult<ScreenState> {
    let s = current_screen(host_id, pane_id).await?;
    if s.prompt.is_some() {
        return Err(http(409, "Answer the open question first"));
    }
    if s.input_visible != Some(true) {
        return Err(http(409, "OpenCode input is not visible. Open the terminal to check."));
    }
    Ok(s)
}

async fn wait_screen(host_id: &str, pane_id: &str, done: impl Fn(&ScreenState) -> bool, ms: u64) -> HubResult<ScreenState> {
    let mut s = current_screen(host_id, pane_id).await?;
    let mut t = 0;
    while !done(&s) && t < ms {
        sleep(150).await;
        s = current_screen(host_id, pane_id).await?;
        t += 150;
    }
    Ok(s)
}

/// OpenCode's own key for the next agent (Tab unless rebound), pressed until the footer names the
/// target: Build, Plan or a custom agent. The footer is what the terminal and the app both show.
async fn open_code_agent(host_id: &str, pane_id: &str, target: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let want = lower(target);
    let keys = opencode_keys_of(&host, Some(&found.pane.cwd)).wait().await;
    let mut start: Option<String> = None;
    for i in 0..12 {
        let s = open_code_ready(host_id, pane_id).await?;
        if s.mode.as_deref() == Some(want.as_str()) {
            nudge(host_id, pane_id);
            return Ok(());
        }
        // back where it started: every agent has been shown
        if i == 0 {
            start = s.mode.clone();
        } else if s.mode == start {
            break;
        }
        send_keys(&host, pane_id, &keys_of(&keys.agent_cycle)).await?;
        wait_screen(host_id, pane_id, |n| n.mode != s.mode, 1200).await?;
    }
    Err(http(409, format!("OpenCode has no {target} agent to switch to")))
}

/// The variant key (Ctrl+T unless rebound) steps through the model's variants and back to the default.
async fn open_code_effort(host_id: &str, pane_id: &str, level: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let want = if level == "default" { None } else { Some(lower(level)) };
    let first = open_code_ready(host_id, pane_id).await?;
    let catalog = open_code_catalog(host_id, pane_id).await?;
    let model = first.model.as_deref().filter(|m| !m.is_empty()).and_then(|m| model_from_footer(&catalog.models, m));
    if let (Some(m), Some(w)) = (model, &want) {
        if !m.efforts.as_ref().is_some_and(|e| e.contains(w)) {
            return Err(http(400, "This OpenCode model does not have that reasoning level"));
        }
    }
    if model.is_some_and(|m| m.efforts.as_ref().is_none_or(|e| e.is_empty())) {
        return Err(http(400, "This OpenCode model has no reasoning levels"));
    }
    let keys = opencode_keys_of(&host, Some(&found.pane.cwd)).wait().await;
    for _ in 0..10 {
        let s = open_code_ready(host_id, pane_id).await?;
        if s.effort == want {
            nudge(host_id, pane_id);
            return Ok(());
        }
        send_keys(&host, pane_id, &keys_of(&keys.variant_cycle)).await?;
        wait_screen(host_id, pane_id, |n| n.effort != s.effort, 1200).await?;
    }
    Err(http(409, "OpenCode did not switch to that reasoning level"))
}

/// Picks a model in OpenCode's own model menu, opened with its key (Ctrl+X M unless rebound) so the
/// message box is left alone: the name goes into the menu's search and Enter takes it. A model with
/// variants then asks for one; the level in use is kept when the model has it, else its default.
/// The footer shows the result, in the terminal and in the app.
async fn open_code_model(host_id: &str, pane_id: &str, id: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let catalog = open_code_catalog(host_id, pane_id).await?;
    let Some(model) = catalog.models.iter().find(|m| m.id == id).cloned() else {
        return Err(http(400, "That model is not offered by OpenCode on this machine"));
    };
    let before = open_code_ready(host_id, pane_id).await?;
    if before.model.as_deref().filter(|m| !m.is_empty()).and_then(|m| model_from_footer(&catalog.models, m)).is_some_and(|m| m.id == id) {
        nudge(host_id, pane_id);
        return Ok(());
    }
    let keys = opencode_keys_of(&host, Some(&found.pane.cwd)).wait().await;
    let keep = before.effort.clone().filter(|e| !e.is_empty() && model.efforts.as_ref().is_some_and(|l| l.contains(e)));
    let on = |s: &ScreenState| s.model.as_deref().filter(|m| !m.is_empty()).and_then(|m| model_from_footer(&catalog.models, m)).is_some_and(|m| m.id == id);
    let stop_driving = start_driving(host_id, pane_id);
    let release = match watchers().peek(host_id, pane_id) {
        Some(w) => Some(w.hold_draft(before.draft.as_deref()).await),
        None => None,
    };
    let r: HubResult<bool> = async {
        send_keys(&host, pane_id, &keys_of(&keys.model_list)).await?;
        let menu = wait_screen(host_id, pane_id, |s| s.input_visible == Some(false), 2500).await?;
        if menu.input_visible != Some(false) {
            return Err(http(409, "OpenCode did not open its model menu. Open the terminal to check."));
        }
        type_into(&host, pane_id, &model.label.split(" · ").skip(1).collect::<Vec<_>>().join(" · "), false).await?;
        sleep(350).await;
        send_keys(&host, pane_id, &["Enter"]).await?;
        let variant = |s: &ScreenState| re!(r"variant", "i").is_match(s.dialog.as_deref().unwrap_or(""));
        let after = wait_screen(host_id, pane_id, |s| (on(s) && s.input_visible != Some(false)) || variant(s), 3000).await?;
        if variant(&after) {
            if let Some(k) = &keep {
                type_into(&host, pane_id, k, false).await?;
                sleep(350).await;
                send_keys(&host, pane_id, &["Enter"]).await?;
            } else {
                send_keys(&host, pane_id, &["Escape"]).await?;
            }
        }
        let done = wait_screen(host_id, pane_id, |s| on(s) && s.input_visible != Some(false), 3000).await?;
        Ok(on(&done) && done.input_visible != Some(false))
    }
    .await;
    let mut picked = *r.as_ref().unwrap_or(&false);
    // the driving flag must end even if the screen can't be read, or prompts stay hidden
    let closing: HubResult<()> = async {
        let now = current_screen(host_id, pane_id).await?;
        if now.input_visible == Some(false) && now.prompt.is_none() {
            send_keys(&host, pane_id, &["Escape"]).await?;
            let closed = wait_screen(host_id, pane_id, |s| s.input_visible != Some(false), 1500).await?;
            picked = picked || (closed.input_visible != Some(false) && on(&closed));
        }
        Ok(())
    }
    .await;
    let _ = closing;
    stop_driving();
    if let Some(release) = release {
        release().await;
    }
    r?;
    if !picked {
        return Err(http(409, "OpenCode did not switch to that model. Open the terminal to check."));
    }
    nudge(host_id, pane_id);
    Ok(())
}

/// A custom command that turned out to open a menu: close it rather than leave the app stuck.
async fn close_open_code_menu(host_id: &str, pane_id: &str) -> HubResult<()> {
    let s = wait_screen(host_id, pane_id, |n| n.input_visible == Some(false), 600).await?;
    if s.input_visible != Some(false) || s.prompt.is_some() {
        return Ok(());
    }
    send_keys(&agent_pane(host_id, pane_id)?.0, pane_id, &["Escape"]).await?;
    Err(http(409, "That command opens a menu in OpenCode's terminal. Open the terminal to use it."))
}

/// `MODE_ALIASES`: what each mode reads as on the agent's screen.
fn mode_alias(target: &str) -> Option<&'static Regex> {
    Some(match target {
        "default" => re!(r"^(manual|default|normal)?$"),
        "acceptEdits" => re!(r"accept"),
        "plan" => re!(r"plan"),
        "auto" => re!(r"^auto"),
        "bypassPermissions" => re!(r"bypass"),
        _ => return None,
    })
}

/// Cycles the permission mode (Shift+Tab) until the target shows on screen (Claude Code, Antigravity).
pub async fn set_mode(host_id: &str, pane_id: &str, target: &str) -> HubResult<()> {
    let (h, p, t) = (host_id.to_owned(), pane_id.to_owned(), target.to_owned());
    in_lane(host_id, pane_id, move || async move { set_pane_mode(&h, &p, &t).await }).await
}

async fn set_pane_mode(host_id: &str, pane_id: &str, target: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let kind = found.pane.agent.as_ref().map(|a| a.kind);
    // OpenCode's agents are its own list (custom ones too); the switch itself checks the target exists
    let Some(kind) = kind.filter(|k| *k == AgentKind::Opencode || k.def().modes.iter().any(|m| m.id == target)) else {
        return Err(http(400, "That permission mode is not supported by this agent"));
    };
    match kind {
        AgentKind::Opencode => {
            if !re!(r"^[\w.-]{1,64}$").is_match(target) {
                return Err(http(400, "Unknown OpenCode agent"));
            }
            return open_code_agent(host_id, pane_id, target).await;
        }
        AgentKind::Codex => return codex_mode(host_id, pane_id, target).await,
        AgentKind::Copilot => {
            if let Some(w) = watchers().peek(host_id, pane_id) {
                w.begin_send().await;
            }
            set_copilot_control(&host, pane_id, &CopilotControl::Permission(target.to_owned())).await?;
            nudge(host_id, pane_id);
            return Ok(());
        }
        AgentKind::Claude | AgentKind::Antigravity => {}
        _ => return Err(http(400, "Mode switching is only supported for Claude Code, Codex and Antigravity")),
    }
    let Some(want) = mode_alias(target) else { return Err(http(400, "Unknown mode")) };
    for _ in 0..7 {
        let s = current_screen(host_id, pane_id).await?;
        if s.prompt.is_some() {
            return Err(http(409, "Answer the open question first"));
        }
        if s.working {
            return Err(http(409, "Change permission mode when the agent finishes working"));
        }
        if kind == AgentKind::Antigravity && s.input_visible != Some(true) {
            return Err(http(409, "Antigravity input is not visible. Open the terminal to check."));
        }
        if s.mode.as_deref().is_some_and(|m| want.is_match(m.jtrim())) || (kind == AgentKind::Claude && s.mode.is_none() && target == "default") {
            nudge(host_id, pane_id);
            return Ok(());
        }
        send_keys(&host, pane_id, &["BTab"]).await?;
        sleep(260).await;
    }
    Err(http(409, "That mode is not available in this session"))
}

// ---------- Codex settings, through its own pickers ----------

fn codex_permission(target: &str) -> Option<&'static str> {
    Some(match target {
        "default" => "Ask for approval",
        "auto-review" => "Approve for me",
        "full-access" => "Full Access",
        "read-only" => "Read Only",
        _ => return None,
    })
}

fn codex_effort(level: &str) -> Option<&'static str> {
    Some(match level {
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        "ultra" => "Ultra",
        _ => return None,
    })
}

fn bare(label: &str) -> String {
    lower(re!(r"\s*\((default|current)\)", "gi").replace_all(label, "").jtrim())
}

/// Waits for a Codex menu whose body matches `title`; None if the composer came back instead.
async fn codex_menu(host_id: &str, pane_id: &str, title: &Regex) -> HubResult<Option<ScreenPrompt>> {
    for i in 0..30 {
        sleep(150).await;
        let s = current_screen(host_id, pane_id).await?;
        if let Some(p) = s.prompt.as_ref().filter(|p| p.body.iter().any(|l| title.is_match(l))) {
            return Ok(Some(p.clone()));
        }
        if i > 12 && s.prompt.is_none() && nonempty(&s.model) {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Moves the highlight from where it is to option `n`, then presses `key`.
async fn codex_choose(host_id: &str, pane_id: &str, p: &ScreenPrompt, n: u64, key: &str) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let from = p.options.iter().find(|o| o.selected).map(|o| o.n as i64).unwrap_or(1);
    let d = n as i64 - from;
    send_keys(&host, pane_id, &repeat(if d > 0 { "Down" } else { "Up" }, d.unsigned_abs() as usize)).await?;
    sleep(120).await;
    send_keys(&host, pane_id, &[key]).await
}

/// The screen once Codex's composer is back (a menu takes a moment to close; Max effort plays a banner), or as it is after `ms`.
async fn codex_composer(host_id: &str, pane_id: &str, ms: u64) -> HubResult<ScreenState> {
    let mut s = current_screen(host_id, pane_id).await?;
    let mut t = 0;
    while !nonempty(&s.model) && s.prompt.is_none() && t < ms {
        sleep(150).await;
        s = current_screen(host_id, pane_id).await?;
        t += 150;
    }
    Ok(s)
}

/// Types a slash command into an idle, empty Codex composer.
async fn codex_command(host_id: &str, pane_id: &str, cmd: &str) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let s = codex_composer(host_id, pane_id, 4000).await?;
    if s.prompt.is_some() {
        return Err(http(409, "Answer the open question first"));
    }
    // no composer on screen means some menu is open; typing into it could pick something
    if !nonempty(&s.model) {
        return Err(http(409, "Codex is showing a menu; close it in the terminal first"));
    }
    if s.working {
        return Err(http(409, "Codex is working; change this when it finishes"));
    }
    if let Some(d) = s.draft.as_deref().filter(|d| !d.is_empty()) {
        clear_input(host_id, pane_id, d).await?;
    }
    type_into(&host, pane_id, cmd, false).await?;
    // an Enter that lands while Codex is still taking in the typing gets lost
    for _ in 0..10 {
        if current_screen(host_id, pane_id).await?.draft.as_deref().map(JsStr::jtrim) == Some(cmd) {
            break;
        }
        sleep(100).await;
    }
    sleep(400).await;
    send_keys(&host, pane_id, &["Enter"]).await
}

/// Runs a slash command that opens a Codex menu, trying once more if the menu doesn't show.
async fn codex_open(host_id: &str, pane_id: &str, cmd: &str, title: &Regex) -> HubResult<ScreenPrompt> {
    for _attempt in 0..2 {
        codex_command(host_id, pane_id, cmd).await?;
        if let Some(menu) = codex_menu(host_id, pane_id, title).await? {
            return Ok(menu);
        }
    }
    Err(http(409, format!("Codex did not open {cmd}")))
}

/// Switches model and/or effort with /model, for this session only ("s" in the effort step; Enter
/// would make it the default for every session). Max and Ultra sit under "More reasoning…".
pub async fn codex_model(host_id: &str, pane_id: &str, model: Option<&str>, effort: Option<&str>) -> HubResult<()> {
    keeping_draft(host_id, pane_id, || pick_codex_model(host_id, pane_id, model, effort)).await
}

/// Runs a settings change that has to use Codex's input box, keeping whatever was typed there: the
/// draft is set aside and typed back once the composer returns, whether the change worked or not.
async fn keeping_draft<T, Fut: Future<Output = HubResult<T>>>(host_id: &str, pane_id: &str, f: impl FnOnce() -> Fut) -> HubResult<T> {
    let stop_driving = start_driving(host_id, pane_id);
    let r = keeping_draft_inner(host_id, pane_id, f).await;
    stop_driving();
    r
}

async fn keeping_draft_inner<T, Fut: Future<Output = HubResult<T>>>(host_id: &str, pane_id: &str, f: impl FnOnce() -> Fut) -> HubResult<T> {
    let before = codex_composer(host_id, pane_id, 0).await?;
    let release = match watchers().peek(host_id, pane_id) {
        Some(w) => Some(w.hold_draft(before.draft.as_deref()).await),
        None => None,
    };
    let draft = before.draft;
    let r = f().await;
    // as a `finally`: an error here takes the place of the result
    let after = codex_composer(host_id, pane_id, 4000).await?;
    if let Some(release) = release {
        release().await;
    } else if let Some(d) = draft.filter(|d| !d.is_empty()) {
        // no watcher (nobody has the chat open): put it back ourselves
        if !nonempty(&after.draft) && nonempty(&after.model) {
            let host = agent_pane(host_id, pane_id)?.0;
            let _ = type_into(&host, pane_id, &d, false).await;
        }
    }
    r
}

async fn pick_codex_model(host_id: &str, pane_id: &str, model: Option<&str>, effort: Option<&str>) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let before = current_screen(host_id, pane_id).await?;
    let models = codex_open(host_id, pane_id, "/model", re!(r"^Select Model")).await?;
    let label = match model {
        Some(m) => {
            let known = codex_models(&host).await.models.into_iter().find(|x| x.id == m);
            Some(known.map(|k| k.label).or_else(|| AgentKind::Codex.def().models.iter().find(|x| x.id == m).map(|x| x.label.clone())).unwrap_or_else(|| m.to_owned()))
        }
        None => None,
    };
    let pick = match (&label, model) {
        (Some(label), Some(m)) => models.options.iter().find(|o| bare(&o.label) == lower(label) || bare(&o.label) == lower(m)),
        _ => models.options.iter().find(|o| o.current == Some(true)),
    };
    let Some(pick) = pick.cloned() else {
        send_keys(&host, pane_id, &["Escape"]).await?;
        return Err(http(400, format!("Codex doesn't offer {} here", label.as_deref().unwrap_or("that model"))));
    };
    codex_choose(host_id, pane_id, &models, pick.n, "Enter").await?;
    let levels = codex_menu(host_id, pane_id, re!(r"^Select Reasoning Level")).await?;
    // a model with one level skips this step
    let Some(levels) = levels else {
        nudge(host_id, pane_id);
        return Ok(());
    };
    // keep the level you had when only the model changes; fall back to the model's default
    let want = lower(effort.or(before.effort.as_deref()).unwrap_or(""));
    let name = codex_effort(&want).map(lower);
    let this_model = label.as_deref().unwrap_or("This model");
    let mut opt = levels.options.iter().find(|o| Some(bare(&o.label)) == name).cloned();
    if opt.is_none() && name.is_some() && (want == "max" || want == "ultra") {
        if let Some(more) = levels.options.iter().find(|o| re!(r"^More reasoning", "i").is_match(&o.label)) {
            codex_choose(host_id, pane_id, &levels, more.n, "Enter").await?;
            let adv = codex_menu(host_id, pane_id, re!(r"^Advanced Reasoning")).await?;
            if let Some(adv) = &adv {
                if let Some(o) = adv.options.iter().find(|x| Some(bare(&x.label)) == name) {
                    codex_choose(host_id, pane_id, adv, o.n, "s").await?;
                    codex_composer(host_id, pane_id, 4000).await?;
                    nudge(host_id, pane_id);
                    return Ok(());
                }
            }
            send_keys(&host, pane_id, &["Escape", "Escape", "Escape"]).await?;
            return Err(http(400, format!("{this_model} has no {} effort", codex_effort(&want).unwrap_or("undefined"))));
        }
    }
    if opt.is_none() && effort.is_none_or(str::is_empty) {
        opt = levels.options.iter().find(|o| re!(r"\(default\)", "i").is_match(&o.label)).or_else(|| levels.options.iter().find(|o| o.current == Some(true))).cloned();
    }
    let Some(opt) = opt else {
        send_keys(&host, pane_id, &["Escape", "Escape"]).await?;
        return Err(http(400, format!("{this_model} has no {} effort", codex_effort(&want).unwrap_or(&want))));
    };
    codex_choose(host_id, pane_id, &levels, opt.n, "s").await?;
    codex_composer(host_id, pane_id, 4000).await?;
    nudge(host_id, pane_id);
    Ok(())
}

/// Rewinds Codex to one of your earlier messages (Esc Esc, then ← until that message is the one
/// highlighted, then Enter): the later turns are dropped and the message comes back into the box to
/// edit. With no text, it picks your last message.
pub async fn codex_rewind(host_id: &str, pane_id: &str, text: Option<&str>) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    if found.pane.agent.as_ref().map(|a| a.kind) != Some(AgentKind::Codex) {
        return Err(http(400, "Only for Codex"));
    }
    let s = codex_composer(host_id, pane_id, 4000).await?;
    if s.prompt.is_some() || !nonempty(&s.model) {
        return Err(http(409, "Codex is showing a menu; close it in the terminal first"));
    }
    if s.working {
        return Err(http(409, "Codex is working; stop it or wait, then rewind"));
    }
    if let Some(d) = s.draft.as_deref().filter(|d| !d.is_empty()) {
        clear_input(host_id, pane_id, d).await?;
    }
    let browse = || async {
        let caps = capture_panes(&host, &[pane_id.to_owned()]).await?;
        Ok::<Option<CodexBrowsing>, HubError>(codex_browsing(caps.get(pane_id).map(Vec::as_slice).unwrap_or(&[])))
    };
    let selected = |b: &Option<CodexBrowsing>| b.as_ref().and_then(|b| b.selected.clone()).filter(|s| !s.is_empty());
    let screen_of = |b: &Option<CodexBrowsing>| b.as_ref().and_then(|b| b.screen.clone());
    send_keys(&host, pane_id, &["Escape"]).await?;
    sleep(200).await;
    send_keys(&host, pane_id, &["Escape"]).await?;
    let mut b = None;
    for _ in 0..10 {
        b = browse().await?;
        if selected(&b).is_some() {
            break;
        }
        sleep(150).await;
    }
    if selected(&b).is_none() {
        if b.is_some() {
            send_keys(&host, pane_id, &["Escape"]).await?;
        }
        return Err(http(409, "Codex has no earlier message to go back to"));
    }
    let want = text.filter(|t| !t.is_empty()).map(|t| squash(t.split('\n').next().unwrap_or("")).to16(60).to_owned());
    let matches = |sel: &str| match &want {
        None => true,
        Some(w) => squash(sel).starts_with(w.as_str()) || w.starts_with(&squash(sel)),
    };
    for _ in 0..80 {
        let sel = selected(&b).unwrap_or_default();
        if matches(&sel) {
            send_keys(&host, pane_id, &["Enter"]).await?;
            nudge(host_id, pane_id);
            return Ok(());
        }
        // ←/→ step between your messages (↑/↓ only scroll)
        send_keys(&host, pane_id, &["Left"]).await?;
        sleep(150).await;
        let before = screen_of(&b);
        b = browse().await?;
        // nothing moved: the oldest message, so it isn't there (two messages can read the same, so
        // compare the whole screen, not just the highlighted text)
        if screen_of(&b) == before {
            sleep(200).await;
            b = browse().await?;
        }
        if selected(&b).is_none() || screen_of(&b) == before {
            break;
        }
    }
    send_keys(&host, pane_id, &["Escape"]).await?;
    Err(http(404, "Couldn't find that message in Codex's history"))
}

/// Plan mode is Shift+Tab; the permission presets are /permissions.
async fn codex_mode(host_id: &str, pane_id: &str, target: &str) -> HubResult<()> {
    keeping_draft(host_id, pane_id, || pick_codex_mode(host_id, pane_id, target)).await
}

async fn pick_codex_mode(host_id: &str, pane_id: &str, target: &str) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let s = current_screen(host_id, pane_id).await?;
    if s.prompt.is_some() {
        return Err(http(409, "Answer the open question first"));
    }
    let in_plan = s.mode.as_deref() == Some("plan");
    if s.working {
        return Err(http(409, "Change permission mode when Codex finishes working"));
    }
    if !nonempty(&s.model) {
        return Err(http(409, "Codex input is not visible. Open the terminal to check."));
    }
    if target == "plan" {
        if !in_plan {
            send_keys(&host, pane_id, &["BTab"]).await?;
        }
        nudge(host_id, pane_id);
        return Ok(());
    }
    let Some(label) = codex_permission(target) else { return Err(http(400, "Unknown mode")) };
    if in_plan {
        send_keys(&host, pane_id, &["BTab"]).await?;
        sleep(250).await;
    }
    let menu = codex_open(host_id, pane_id, "/permissions", re!(r"Permissions$")).await?;
    let Some(opt) = menu.options.iter().find(|o| bare(&o.label) == lower(label)) else {
        send_keys(&host, pane_id, &["Escape"]).await?;
        return Err(http(400, format!("Codex doesn't offer \"{label}\" here")));
    };
    if opt.current != Some(true) {
        codex_choose(host_id, pane_id, &menu, opt.n, "Enter").await?;
    } else {
        send_keys(&host, pane_id, &["Escape"]).await?;
    }
    codex_composer(host_id, pane_id, 4000).await?;
    registry().forget_status(host_id);
    nudge(host_id, pane_id);
    Ok(())
}

/// The command `launch_bin` runs to find an agent's binary.
pub fn launch_bin_command(bin: &str) -> String {
    format!(r#"b={}; if "${{SHELL:-/bin/sh}}" -lic 'command -v "$0"' "$b" </dev/null >/dev/null 2>&1; then echo "$b"; else {PATHS}; command -v "$b" || echo "$b"; fi"#, shq(bin))
}

/// The agent's command as the pane's shell will find it: the plain name when it is on that shell's
/// PATH, otherwise the full path the hub found (e.g. installed under another nvm Node version).
async fn launch_bin(host: &Host, launch: &str) -> HubResult<String> {
    let bin = launch.split(' ').next().unwrap_or(launch);
    let r = host.exec(&launch_bin_command(bin), ExecOptions::timeout(10_000)).await?;
    let found = r.stdout.jtrim().rsplit('\n').next().filter(|f| !f.is_empty()).unwrap_or(bin);
    Ok(if found == bin || !found.starts_with('/') {
        bin.to_owned()
    } else if re!(r"^[\w./-]+$").is_match(found) {
        found.to_owned()
    } else {
        shq(found)
    })
}

/// `cmd` with its first word (the agent's command) replaced by `bin`.
fn with_bin(cmd: &str, bin: &str) -> String {
    re!(r"^\S+").replacen(cmd, 1, regex::NoExpand(bin)).into_owned()
}

/// No session (start fresh), or a conversation id, nothing a shell would read as more: a UUID, or
/// for OpenCode its own "ses_…" form.
pub fn is_session_id(id: Option<&Value>, agent: Option<AgentKind>) -> bool {
    match id {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => is_session_id_str(Some(s), agent),
        _ => false,
    }
}

pub fn is_session_id_str(id: Option<&str>, agent: Option<AgentKind>) -> bool {
    match id {
        None | Some("") => true,
        Some(s) => {
            re!(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", "i").is_match(s)
                || (agent == Some(AgentKind::Opencode) && re!(r"^ses_[A-Za-z0-9]{1,64}$").is_match(s))
        }
    }
}

/// An agent named on the wire ("claude"), or None for anything else.
pub fn agent_kind(name: &str) -> Option<AgentKind> {
    AGENT_KINDS.into_iter().find(|k| k.as_str() == name)
}

/// Starts (or resumes) an agent in a pane that is sitting at a shell prompt.
pub async fn start_agent(host_id: &str, pane_id: &str, agent: Option<&Value>, resume: Option<&Value>) -> HubResult<()> {
    // both end up typed into a shell
    let kind = match agent {
        None | Some(Value::Null) => Some(AgentKind::Claude),
        Some(Value::String(s)) => agent_kind(s),
        _ => None,
    };
    let Some(kind) = kind else { return Err(http(400, "Unknown agent")) };
    if !is_session_id(resume, Some(kind)) {
        return Err(http(400, "Bad session id"));
    }
    let (_, found) = agent_pane(host_id, pane_id)?;
    if found.pane.agent.is_some() {
        return Err(http(409, "An agent is already running here"));
    }
    type_start(host_id, pane_id, kind, resume.and_then(Value::as_str)).await
}

/// Types the command that starts the agent (resuming `resume`, if given) at the pane's shell
/// prompt. The caller has checked the agent and the id, and that the pane is at its shell.
pub async fn type_start(host_id: &str, pane_id: &str, agent: AgentKind, resume: Option<&str>) -> HubResult<()> {
    let (host, _) = agent_pane(host_id, pane_id)?;
    let def = agent.def();
    let cmd = match resume.filter(|r| !r.is_empty()) {
        None => def.launch.clone(),
        Some(r) => match agent {
            AgentKind::Codex => format!("codex resume {r}"),
            AgentKind::Antigravity => format!("agy --conversation {r}"),
            AgentKind::Opencode => format!("{} --session {r}", def.launch),
            _ => format!("{} --resume {r}", def.launch),
        },
    };
    let line = with_bin(&cmd, &launch_bin(&host, &def.launch).await?);
    type_into(&host, pane_id, &line, true).await?;
    registry().refresh(host_id, 1500);
    Ok(())
}

/// Reconnects a Claude session's live numbers without restarting it (see reconnect_status_line).
pub async fn reconnect_agent(host_id: &str, pane_id: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    if found.pane.agent.as_ref().map(|a| a.kind) != Some(AgentKind::Claude) {
        return Err(http(400, "Only Claude Code sessions report live numbers"));
    }
    let cwd = match registry().agent_proc(host_id, pane_id).and_then(|p| p.cwd) {
        Some(c) => c,
        None => found.pane.cwd.clone(),
    };
    if cwd.is_empty() {
        return Err(http(409, "Unknown working folder"));
    }
    super::integration::reconnect_status_line(&host, &cwd).await?;
    registry().refresh(host_id, 3000);
    Ok(())
}

/// Stops the current turn. Esc only while the spinner is up (a stray double Esc on an idle Claude
/// opens the rewind menu), then waits for the spinner to go and tries once more if it hasn't.
pub async fn interrupt(host_id: &str, pane_id: &str) -> HubResult<()> {
    // one stop at a time per pane, and none within a second of the last: a second Esc that lands
    // after the agent stopped opens Claude Code's rewind menu or clears the box
    let k = format!("{host_id}\u{1}{pane_id}");
    let now = now_ms();
    let busy = STOPPING.with(|s| {
        let mut s = s.borrow_mut();
        match s.get(&k) {
            Some(at) if now.saturating_sub(*at) < INTERRUPT_GAP_MS => true,
            _ => {
                // still running counts as just now
                s.insert(k.clone(), u64::MAX);
                false
            }
        }
    });
    if busy {
        return Ok(());
    }
    let r = interrupt_now(host_id, pane_id).await;
    let end = now_ms();
    STOPPING.with(|s| s.borrow_mut().insert(k, end));
    r
}

/// How long after a stop another is ignored.
pub const INTERRUPT_GAP_MS: u64 = 1000;

async fn interrupt_now(host_id: &str, pane_id: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let agent = found.pane.agent.as_ref();
    let is_open_code = agent.is_some_and(|a| a.kind == AgentKind::Opencode);
    if let Some(uri) = agent.and_then(|a| a.transcript_path.as_deref()).filter(|u| is_open_code && via_open_code_api(Some(u))) {
        OpenCodeClient::new(&host, uri, Some(&found.pane.cwd))?.abort().await?;
        nudge(host_id, pane_id);
        return Ok(());
    }
    // OpenCode stops on the second press of its interrupt key ("esc again to interrupt"), Esc unless rebound
    let stop: Vec<String> = if is_open_code {
        let k = opencode_keys_of(&host, Some(&found.pane.cwd)).wait().await;
        k.session_interrupt.iter().chain(k.session_interrupt.iter()).cloned().collect()
    } else {
        vec!["Escape".into()]
    };
    for _attempt in 0..2 {
        if !current_screen(host_id, pane_id).await?.working {
            break;
        }
        send_keys(&host, pane_id, &keys_of(&stop)).await?;
        for _ in 0..8 {
            sleep(250).await;
            if !current_screen(host_id, pane_id).await?.working {
                nudge(host_id, pane_id);
                registry().refresh(host_id, 100);
                return Ok(());
            }
        }
    }
    nudge(host_id, pane_id);
    registry().refresh(host_id, 100);
    Ok(())
}

/// Flags that take a value, per agent; everything else positional (an initial prompt) is dropped.
fn value_flags(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &[
            "--model",
            "--permission-mode",
            "--add-dir",
            "--mcp-config",
            "--settings",
            "--allowedTools",
            "--allowed-tools",
            "--disallowedTools",
            "--disallowed-tools",
            "--fallback-model",
            "--agent",
            "--effort",
            "--plugin-dir",
        ],
        AgentKind::Codex => &["-m", "--model", "-c", "--config", "-p", "--profile", "-a", "--ask-for-approval", "-s", "--sandbox", "-C", "--cd"],
        AgentKind::Antigravity => &["--model", "--effort", "--mode", "--add-dir", "--agent", "--project", "--log-file"],
        _ => &[],
    }
}

fn drop_flags(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &["--resume", "-r", "--continue", "-c", "--session-id", "--fork-session", "-p", "--print"],
        AgentKind::Antigravity => &[
            "--conversation",
            "--continue",
            "-c",
            "-p",
            "--print",
            "--prompt",
            "-i",
            "--prompt-interactive",
            "--new-project",
            "--print-timeout",
            "--output-format",
            "--input-format",
            "--json-schema",
        ],
        _ => &[],
    }
}

/// The command that reopens this conversation with the flags the agent was started with, from
/// `ps` text: that loses where a value with spaces in it ends, so it's split on spaces as a last
/// resort ([`resume_command_argv`] takes the real argument list).
pub fn resume_command(kind: AgentKind, session_id: Option<&str>, args: &str) -> String {
    let tokens: Vec<String> = crate::js::re::split_plain(args.jtrim(), re!(r"\s+")).into_iter().map(str::to_owned).collect();
    resume_command_argv(kind, session_id, &tokens)
}

/// `resumeCommand` with the real argument list where the machine can give it.
pub fn resume_command_argv(kind: AgentKind, session_id: Option<&str>, tokens: &[String]) -> String {
    // a terminal-first agent's flags aren't known, so which word is a value or a first message
    // isn't either: it starts again plain rather than repeat a task
    if !kind.chat() {
        return kind.def().launch.clone();
    }
    // skip the interpreter and binary ("node /…/codex.js", "/…/claude")
    let name = if kind == AgentKind::Antigravity { "agy" } else { kind.as_str() };
    let bin_re = dynamic(&format!(r"(^|/){name}(\.js)?$"), "");
    let mut rest: Vec<&String> = match tokens.iter().position(|t| bin_re.is_match(t)) {
        Some(i) => tokens[i + 1..].iter().collect(),
        None => tokens.iter().collect(),
    };
    if kind == AgentKind::Codex && rest.first().is_some_and(|r| *r == "resume") {
        let n = if rest.get(1).is_some_and(|r| re!(r"^[0-9a-f-]{36}$").is_match(r)) { 2 } else { 1 };
        rest.drain(..n);
    }
    let takes = value_flags(kind);
    let drop = drop_flags(kind);
    let mut keep: Vec<String> = vec![];
    let mut i = 0;
    while i < rest.len() {
        let t = rest[i];
        let flag = t.split('=').next().unwrap_or("");
        if !t.starts_with('-') {
            i += 1;
            continue;
        }
        let next_ok = rest.get(i + 1).is_some_and(|n| !n.is_empty() && !n.starts_with('-'));
        let has_value = takes.contains(&flag) && !t.contains('=') && next_ok;
        let drop_value = drop.contains(&flag) && !t.contains('=') && next_ok && flag != "--fork-session";
        if drop.contains(&flag) {
            i += if drop_value { 2 } else { 1 };
            continue;
        }
        keep.push(shq(t));
        if has_value {
            i += 1;
            keep.push(shq(rest[i]));
        }
        i += 1;
    }
    let base = kind.def().launch.clone();
    let mut parts = vec![base];
    // no conversation yet (nothing was said): start fresh with the same flags
    if let Some(s) = session_id.filter(|s| !s.is_empty()) {
        match kind {
            AgentKind::Codex => parts.extend(["resume".to_owned(), s.to_owned()]),
            AgentKind::Antigravity => parts.extend(["--conversation".to_owned(), s.to_owned()]),
            _ => parts.extend(["--resume".to_owned(), s.to_owned()]),
        }
    }
    parts.extend(keep);
    parts.join(" ")
}

/// A process's arguments as it was given them (Linux), or None where there's no /proc.
pub async fn argv(host: &Host, pid: f64) -> Option<Vec<String>> {
    let r = host.run(&format!(r"tr '\0' '\n' < /proc/{}/cmdline 2>/dev/null", fmt_number(pid.trunc()))).await.ok()?;
    let args: Vec<String> = if r.code == 0 { r.stdout.strip_suffix('\n').unwrap_or(&r.stdout).split('\n').map(str::to_owned).collect() } else { vec![] };
    (args.first().is_some_and(|a| !a.is_empty())).then_some(args)
}

/// The resume command for an agent process: from its real arguments, else from `ps`'s.
async fn resume_for(host: &Host, kind: AgentKind, session_id: Option<&str>, pid: f64, ps_args: &str) -> String {
    match argv(host, pid).await {
        Some(a) => resume_command_argv(kind, session_id, &a),
        None => resume_command(kind, session_id, ps_args),
    }
}

async fn saved_transcript(host: &Host, path: Option<&str>) -> HubResult<bool> {
    let Some(p) = path.filter(|p| !p.is_empty()) else { return Ok(false) };
    Ok(host.run(&format!("test -s {} && echo yes", shq(p))).await?.stdout.contains("yes"))
}

/// Background work the screen lists under the box, when it lists any.
fn background_of(s: &ScreenState) -> Option<&str> {
    s.background.as_ref().and_then(|b| b.as_deref()).filter(|b| !b.is_empty())
}

/// Exits the agent and reopens the same conversation in the same pane, e.g. so it picks up
/// settings that changed under it. Refuses while it's busy, since a restart would cut that off.
pub async fn restart_agent(host_id: &str, pane_id: &str) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let Some(a) = found.pane.agent.clone().filter(|a| matches!(a.kind, AgentKind::Claude | AgentKind::Codex | AgentKind::Antigravity)) else {
        return Err(http(400, "Only Claude Code, Codex and Antigravity sessions can be resumed"));
    };
    let Some(session) = a.session_id.clone().filter(|s| re!(r"^[0-9a-f-]{36}$").is_match(s)) else {
        return Err(http(409, "No conversation to resume yet"));
    };
    let Some(proc) = registry().agent_proc(host_id, pane_id) else { return Err(http(409, "The agent process is gone")) };
    let s = current_screen(host_id, pane_id).await?;
    if s.working {
        return Err(http(409, "It is working; stop it or wait, then restart"));
    }
    if s.prompt.is_some() {
        return Err(http(409, "Answer the open question first"));
    }
    if let Some(bg) = background_of(&s) {
        return Err(http(409, format!("Restarting would stop {bg} running in the background")));
    }
    let saved = saved_transcript(&host, a.transcript_path.as_deref()).await?;
    let cmd = resume_for(&host, a.kind, saved.then_some(session.as_str()), proc.pid, &proc.args).await;
    let p = shq(pane_id);
    // started as the pane's own command: keep the pane open across the exit, then respawn it
    let direct = proc.pid == proc.pane_pid;
    let win = on_psmux(host_id);
    if direct {
        host.run(&remain_on_exit(win, &p, true)).await?;
    }
    // the "/exit" typed below is Inyon's, not a draft to keep
    let w = watchers().peek(host_id, pane_id);
    if let Some(w) = &w {
        w.begin_send().await;
    }
    let r: HubResult<()> = async {
        send_keys(&host, pane_id, &["C-u"]).await?;
        type_into(&host, pane_id, exit_command(a.kind), true).await?;
        let mut gone = false;
        let mut i = 0;
        while i < 40 && !gone {
            sleep(250).await;
            gone = !host.run(&format!("kill -0 {} 2>/dev/null && echo alive", fmt_number(proc.pid))).await?.stdout.contains("alive");
            i += 1;
        }
        if !gone {
            return Err(http(504, "It did not exit; restart it from the terminal"));
        }
        if direct {
            let cwd = proc.cwd.as_deref().filter(|c| !c.is_empty()).map(|c| format!("-c {}", shq(&tmux_literal(c, win)))).unwrap_or_default();
            pane_dead(&host, &p).await?;
            host.run(&format!("tmux respawn-pane {}-t {p} {cwd} {}", if win { "-k " } else { "" }, shq(&cmd))).await?;
        } else {
            sleep(400).await;
            let line = with_bin(&cmd, &launch_bin(&host, &a.kind.def().launch).await?);
            type_into(&host, pane_id, &line, true).await?;
        }
        Ok(())
    }
    .await;
    // as a `finally`: an error here takes the place of the result
    if let Some(w) = &w {
        w.draft_sent(None, 0);
    }
    if direct {
        host.run(&remain_on_exit(win, &p, false)).await?;
    }
    r?;
    registry().refresh(host_id, 1500);
    Ok(())
}

/// psmux (tmux on Windows) keeps remain-on-exit on the window rather than the pane, and its panes
/// run PowerShell, not $SHELL.
fn on_psmux(host_id: &str) -> bool {
    registry().get(host_id).is_some_and(|r| r.borrow().state.os.as_deref() == Some("Windows"))
}

pub fn remain_on_exit(win: bool, p: &str, on: bool) -> String {
    format!("tmux set-option {} -t {p} {}", if win { "-w" } else { "-p" }, if on { "remain-on-exit on" } else { "-u remain-on-exit" })
}

/// The command that gives a pane its shell back once the agent it ran has exited.
pub fn respawn_shell(win: bool, p: &str, cwd: Option<&str>) -> String {
    let c = cwd.filter(|c| !c.is_empty()).map(|c| format!("-c {}", shq(&tmux_literal(c, win)))).unwrap_or_default();
    if win {
        format!(r#"tmux respawn-pane -k -t {p} {c} "$(command -v pwsh.exe >/dev/null 2>&1 && echo pwsh.exe || echo powershell.exe)""#)
    } else {
        format!(r#"tmux respawn-pane -t {p} {c} "${{SHELL:-/bin/sh}}" -l"#)
    }
}

/// tmux marks a pane dead a moment after its process is gone, and refuses to respawn it before that.
async fn pane_dead(host: &Host, p: &str) -> HubResult<()> {
    for _ in 0..20 {
        // display answers even for a pane that's gone, with nothing: that's as done as dead
        let out = host.run(&format!("tmux display -p -t {p} '#{{pane_id}} #{{pane_dead}}' 2>/dev/null")).await?.stdout;
        let mut it = out.jtrim().split(' ');
        let id = it.next().unwrap_or("");
        if id.is_empty() || it.next() == Some("1") {
            return Ok(());
        }
        sleep(100).await;
    }
    Ok(())
}

/// Each agent's own way out.
fn exit_command(kind: AgentKind) -> &'static str {
    if kind == AgentKind::Gemini {
        "/quit"
    } else {
        "/exit"
    }
}

/// How an exit checks on the agent and its pane.
struct Exiting<'a> {
    host_id: &'a str,
    host: &'a Host,
    p: String,
    pid: Option<f64>,
    cwd: Option<String>,
    end: bool,
    direct: bool,
    win: bool,
    w: Option<Rc<super::watcher::ChatWatcher>>,
}

impl Exiting<'_> {
    /// ending the session closes the pane after the agent, and anything still running in it
    async fn close(&self) -> HubResult<()> {
        self.host.run(&format!("tmux kill-pane -t {} 2>/dev/null; true", self.p)).await?;
        registry().refresh(self.host_id, 300);
        Ok(())
    }

    /// An agent that was its pane's own command takes the pane with it, and kill -0 can still answer
    /// for its pid a while after (a process not yet reaped), so a pane that's gone counts as gone too.
    /// tmux display exits 0 for a pane that no longer exists and prints nothing, so the id it prints
    /// is what says the pane is there.
    async fn alive(&self) -> HubResult<bool> {
        let p = &self.p;
        let pid = self.pid.map(|pid| format!(" && kill -0 {} 2>/dev/null", fmt_number(pid))).unwrap_or_default();
        Ok(self.host.run(&format!(r#"[ "$(tmux display -p -t {p} '#{{pane_id}}' 2>/dev/null)" = {p} ]{pid} && echo alive"#)).await?.stdout.contains("alive"))
    }

    async fn wait_gone(&self, ms: u64) -> HubResult<bool> {
        let mut t = 0;
        while t < ms {
            sleep(250).await;
            if !self.alive().await? {
                return Ok(true);
            }
            t += 250;
        }
        Ok(false)
    }

    async fn done(&self) -> HubResult<()> {
        if let Some(w) = &self.w {
            w.draft_sent(None, 0);
        }
        if self.end {
            return self.close().await;
        }
        if self.direct {
            pane_dead(self.host, &self.p).await?;
            // with no command tmux would rerun the agent; ask for the login shell instead
            self.host.run(&format!("{}; {}", respawn_shell(self.win, &self.p, self.cwd.as_deref()), remain_on_exit(self.win, &self.p, false))).await?;
        }
        registry().refresh(self.host_id, 300);
        Ok(())
    }

    /// An agent that leaves the moment it is asked takes its pane with it, and the keystroke being
    /// checked on screen then finds no pane: that is the exit, not a failure.
    async fn left(&self, err: &HubError) -> HubResult<bool> {
        Ok(re!(r"can't find pane|no such pane", "i").is_match(&err.message) && !self.alive().await?)
    }
}

/// Quits the agent and leaves the pane at its shell (or closes it, if the agent was the pane's own
/// command). Stops a running turn first and backs out of any dialog, then asks the agent to exit;
/// Ctrl+C is the fallback when that doesn't work.
pub async fn exit_agent(host_id: &str, pane_id: &str, end: bool) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let Some(a) = found.pane.agent.clone() else { return Err(http(409, "No agent is running in this pane")) };
    let p = shq(pane_id);
    let proc = registry().agent_proc(host_id, pane_id);
    let s = current_screen(host_id, pane_id).await?;
    if let Some(bg) = background_of(&s).filter(|_| !end) {
        return Err(http(409, format!("Exiting would stop {bg} running in the background")));
    }
    if s.working {
        interrupt(host_id, pane_id).await?;
    }
    // a dialog (permission, folder trust): back out of it before typing the command
    if s.prompt.as_ref().is_some_and(|p| p.api != Some(true)) {
        send_keys(&host, pane_id, &["Escape"]).await?;
        sleep(300).await;
    }
    // started as the pane's own command (`tmux new agy`): keep the pane, and give it a shell after
    let direct = proc.as_ref().is_some_and(|pr| pr.pid == pr.pane_pid) && !end;
    let win = on_psmux(host_id);
    if direct {
        host.run(&remain_on_exit(win, &p, true)).await?;
    }
    // the "/exit" and Ctrl+C below are Inyon's, not a draft to keep
    let w = watchers().peek(host_id, pane_id);
    if let Some(w) = &w {
        w.begin_send().await;
    }
    let x = Exiting { host_id, host: &host, p, pid: proc.as_ref().map(|pr| pr.pid), cwd: proc.as_ref().and_then(|pr| pr.cwd.clone()), end, direct, win, w };
    if current_screen(host_id, pane_id).await?.prompt.is_none() {
        let typed: HubResult<()> = async {
            send_keys(&host, pane_id, &["C-u"]).await?;
            type_into(&host, pane_id, exit_command(a.kind), true).await
        }
        .await;
        if let Err(e) = typed {
            if !x.left(&e).await? {
                return Err(e);
            }
            return x.done().await;
        }
        if x.wait_gone(5000).await? {
            return x.done().await;
        }
    }
    for _ in 0..2 {
        let pressed: HubResult<()> = async {
            send_keys(&host, pane_id, &["C-c"]).await?;
            sleep(300).await;
            send_keys(&host, pane_id, &["C-c"]).await
        }
        .await;
        if let Err(e) = pressed {
            if !x.left(&e).await? {
                return Err(e);
            }
            return x.done().await;
        }
        if x.wait_gone(2500).await? {
            return x.done().await;
        }
    }
    if let Some(w) = &x.w {
        w.draft_sent(None, 0);
    }
    // it wouldn't go: closing its pane ends it anyway
    if end {
        return x.close().await;
    }
    if direct {
        host.run(&remain_on_exit(win, &x.p, false)).await?;
    }
    registry().refresh(host_id, 300);
    Err(http(504, format!("{} did not exit; close it from the terminal", name_of(a.kind))))
}

/// What closing a pane takes with it: tmux closes a window with its last pane, a session with its last window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Closes {
    Pane,
    Window,
    Session,
}

pub fn closes(found: &FoundPane) -> Closes {
    if found.window.panes.len() > 1 {
        return Closes::Pane;
    }
    if found.session.windows.len() > 1 {
        Closes::Window
    } else {
        Closes::Session
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndSession {
    pub closed: Closes,
}

/// Ends a session for good. The agent exits the way /exit would (a running turn is stopped first,
/// a dialog backed out of), then its tmux pane is killed. Only that pane: its window goes too only
/// when it was the last pane there, and the tmux session only when that was its last window.
pub async fn end_session(host_id: &str, pane_id: &str) -> HubResult<EndSession> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let closed = closes(&found);
    if found.pane.agent.is_some() {
        exit_agent(host_id, pane_id, true).await?;
    } else {
        host.run(&format!("tmux kill-pane -t {} 2>/dev/null; true", shq(pane_id))).await?;
        registry().refresh(host_id, 300);
    }
    Ok(EndSession { closed })
}

/// A new window or session, as the app asks for one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LaunchOpts {
    pub session: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    /// an agent kind, or "shell"
    pub agent: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub prompt: Option<String>,
    /// a custom agent's id (Settings › Agents), started in place of `agent`
    pub custom: Option<String>,
    /// a terminal-first agent's sign-in (its `login` command) rather than the agent itself
    #[serde(default)]
    pub sign_in: bool,
    /// more flags for the agent's command, from the hub itself (the companion's)
    #[serde(skip)]
    pub extra: Vec<String>,
}

/// zsh's first-run questions in a home with no ~/.zshrc; "q" leaves it for next time and changes nothing
pub fn zsh_newuser() -> &'static Regex {
    re!(r"zsh-newuser-install|Type one of the keys in parentheses")
}

/// Waits for a new pane's shell to show its prompt before typing a command into it: a slow
/// startup (oh-my-zsh, nvm) or a question the shell asks first would otherwise take the keys.
pub async fn shell_ready(host: &Host, pane_id: &str, ms: u64) -> HubResult<()> {
    let end = now_ms() + ms;
    let mut last = String::new();
    let mut stable_since = now_ms();
    let mut skipped = false;
    sleep(150).await;
    while now_ms() < end {
        let lines = capture_panes(host, &[pane_id.to_owned()]).await.unwrap_or_default().get(pane_id).cloned().unwrap_or_default();
        let text = re!(r"\x1b\[[0-9;]*m", "g").replace_all(&lines.join("\n"), "").into_owned();
        // the question stays on screen after it's answered, so press q once and wait for the prompt below it
        if !skipped && zsh_newuser().is_match(&text) {
            skipped = true;
            send_keys(host, pane_id, &["q"]).await?;
            sleep(300).await;
            continue;
        }
        let tail = text.jtrim_end().rsplit('\n').next().unwrap_or("");
        // a prompt ends in one of these, usually with a space after it
        if re!(r"[$%#>❯➜»]\s*$").is_match(tail) {
            return Ok(());
        }
        if text != last {
            last = text;
            stable_since = now_ms();
        } else if !text.is_blank() && now_ms() - stable_since > 800 {
            return Ok(());
        }
        sleep(150).await;
    }
    Ok(())
}

/// New tmux window (in an existing or new session) at `cwd`, optionally starting an agent.
pub async fn launch(host_id: &str, opts: &LaunchOpts) -> HubResult<String> {
    let host = registry().host(host_id)?;
    // before anything is created: an unknown agent would fail after the window exists
    let custom = match opts.custom.as_deref().filter(|c| !c.is_empty()) {
        Some(id) => Some(crate::agents::custom::by_id(id).ok_or_else(|| http(400, "Unknown custom agent"))?),
        None => None,
    };
    let agent_name = match &custom {
        Some(c) => Some(c.name.as_str()),
        None => opts.agent.as_deref().filter(|a| !a.is_empty() && *a != "shell"),
    };
    let agent = match (&custom, agent_name) {
        (Some(c), _) => Some(c.like),
        (None, Some(a)) => Some(agent_kind(a).ok_or_else(|| http(400, "Unknown agent"))?),
        (None, None) => None,
    };
    // a terminal-first agent has no way Inyon knows to hand it a first message
    if let Some(a) = agent.filter(|a| !a.chat() && opts.prompt.as_deref().is_some_and(|p| !p.is_blank())) {
        return Err(http(400, format!("{} starts without a first message. Type it in its terminal once it's open.", a.def().name)));
    }
    let cwd = opts.cwd.as_deref().map(JsStr::jtrim).filter(|c| !c.is_empty()).unwrap_or("~");
    let home = cwd == "~" || cwd.starts_with("~/");
    let dir = if home { format!("\"$HOME\"{}", shq(&cwd[1..])) } else { shq(cwd) };
    // the same folder as tmux reads it, where `#` would start a format
    let win = on_psmux(host_id);
    let tdir = if home { format!("\"$HOME\"{}", shq(&tmux_literal(&cwd[1..], win))) } else { shq(&tmux_literal(cwd, win)) };
    let fmt = shq("#{pane_id}");
    let cmd = match opts.session.as_deref().filter(|s| !s.is_empty()) {
        Some(session) => format!("tmux new-window -t {} -c {tdir} -P -F {fmt}", shq(session)),
        None => {
            let raw = opts.name.as_deref().map(JsStr::jtrim).filter(|n| !n.is_empty()).or(agent_name).unwrap_or("shell");
            // tmux won't take . or : in a session name, and # would start a format
            let q = shq(&raw.replace(['.', ':', '#'], "-"));
            // a second "claude" becomes "claude-2" rather than failing on the taken name
            format!(r#"n={q}; i=2; while tmux has-session -t "=$n" 2>/dev/null; do n={q}-$i; i=$((i+1)); done; tmux new-session -d -s "$n" -c {tdir} -P -F {fmt}"#)
        }
    };
    let shown = cwd.replace(['"', '$', '`', '\\'], "");
    let r = host.run(&format!(r#"[ -d {dir} ] || {{ echo "No such folder: {shown}" >&2; exit 3; }}; {cmd}"#)).await?;
    let pane_id = r.stdout.jtrim().to_owned();
    if r.code != 0 || !pane_id.starts_with('%') {
        let err = r.stderr.jtrim();
        return Err(http(400, if err.is_empty() { "Could not create window" } else { err }));
    }
    if let Some(agent) = agent {
        let sign_in = opts.sign_in && custom.is_none() && !agent.chat();
        let mut parts = vec![if sign_in { agent.def().login.clone() } else { custom.as_ref().map_or_else(|| agent.def().launch.clone(), |c| c.command.clone()) }];
        // OpenCode names models "provider/model"
        if let Some(model) = opts.model.as_deref().filter(|m| if agent == AgentKind::Opencode { re!(r"^[\w.:-]+/[\w.:/-]+$").is_match(m) } else { re!(r"^[\w.:-]+$").is_match(m) })
        {
            parts.push(match agent {
                AgentKind::Codex => format!("-m {model}"),
                AgentKind::Antigravity => format!("--model {}", agy_model_id(model, None)),
                _ => format!("--model {model}"),
            });
        }
        let mode = opts.mode.as_deref().unwrap_or("");
        if agent == AgentKind::Claude && re!(r"^\w+$").is_match(mode) {
            parts.push(format!("--permission-mode {mode}"));
        }
        if agent == AgentKind::Antigravity && (mode == "acceptEdits" || mode == "plan") {
            parts.push(format!("--mode {}", if mode == "plan" { "plan" } else { "accept-edits" }));
        }
        parts.extend(opts.extra.iter().cloned());
        // files you attach land in ~/.inyon/uploads; without this agy asks before reading each one
        if agent == AgentKind::Antigravity {
            host.run(r#"mkdir -p "$HOME/.inyon/uploads""#).await?;
            parts.push(r#"--add-dir "$HOME/.inyon/uploads""#.into());
        }
        // the first message: Claude and Codex take it as an argument; OpenCode's argument is a folder
        // (--prompt instead), and agy and Copilot use -i (runs it and stays interactive)
        if let Some(prompt) = opts.prompt.as_deref().filter(|p| !p.is_blank()) {
            let text = shq(prompt.jtrim());
            parts.push(match agent {
                AgentKind::Antigravity | AgentKind::Copilot | AgentKind::Gemini => format!("-i {text}"),
                AgentKind::Opencode => format!("--prompt {text}"),
                _ => text,
            });
        }
        parts[0] = with_bin(&parts[0], &launch_bin(&host, &parts[0]).await?);
        shell_ready(&host, &pane_id, 8000).await?;
        type_into(&host, &pane_id, &parts.join(" "), true).await?;
    }
    registry().refresh(host_id, if agent.is_some() { 1500 } else { 100 });
    Ok(pane_id)
}

/// Opens a copy of the conversation in a new tmux window next to this one, in the same folder and
/// with the same flags: both carry on from here separately. Claude Code, Codex and OpenCode fork
/// a conversation themselves; this only starts them that way.
pub async fn fork_conversation(host_id: &str, pane_id: &str) -> HubResult<String> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    let Some(a) = found.pane.agent.clone().filter(|a| matches!(a.kind, AgentKind::Claude | AgentKind::Codex | AgentKind::Opencode)) else {
        return Err(http(400, "Only Claude Code, Codex and OpenCode conversations can be forked"));
    };
    let Some(session) = a.session_id.clone().filter(|s| !s.is_empty() && is_session_id_str(Some(s), Some(a.kind))) else {
        return Err(http(409, "No conversation to fork yet"));
    };
    if a.kind != AgentKind::Opencode && !saved_transcript(&host, a.transcript_path.as_deref()).await? {
        return Err(http(409, "Say something first; there is nothing to fork yet"));
    }
    let proc = registry().agent_proc(host_id, pane_id);
    let cwd = match proc.as_ref().and_then(|p| p.cwd.clone()) {
        Some(c) => c,
        None => found.pane.cwd.clone(),
    };
    let resume = |kind| {
        let host = host.clone();
        let proc = proc.clone();
        let session = session.clone();
        async move {
            match &proc {
                Some(p) => resume_for(&host, kind, Some(&session), p.pid, &p.args).await,
                None => resume_command(kind, Some(&session), &kind.def().launch),
            }
        }
    };
    let cmd = match a.kind {
        AgentKind::Claude => format!("{} --fork-session", resume(AgentKind::Claude).await),
        AgentKind::Codex => re!(r"^(\S+) resume ").replacen(&resume(AgentKind::Codex).await, 1, "$1 fork ").into_owned(),
        _ => format!("{} --session {session} --fork", AgentKind::Opencode.def().launch),
    };
    let r = host.run(&format!("tmux new-window -t {} -c {} -P -F {}", shq(&found.session.name), shq(&tmux_literal(&cwd, on_psmux(host_id))), shq("#{pane_id}"))).await?;
    let new_pane = r.stdout.jtrim().to_owned();
    if r.code != 0 || !new_pane.starts_with('%') {
        let err = r.stderr.jtrim();
        return Err(http(500, if err.is_empty() { "Could not open a window" } else { err }));
    }
    shell_ready(&host, &new_pane, 8000).await?;
    let line = with_bin(&cmd, &launch_bin(&host, &a.kind.def().launch).await?);
    type_into(&host, &new_pane, &line, true).await?;
    registry().refresh(host_id, 1500);
    Ok(new_pane)
}

/// Turns Remote Control on, or off (opens the /remote-control menu and picks Disconnect).
pub async fn remote_control(host_id: &str, pane_id: &str, on: bool) -> HubResult<()> {
    let (_, found) = agent_pane(host_id, pane_id)?;
    match found.pane.agent.as_ref().map(|a| a.kind) {
        Some(AgentKind::Antigravity) => return send_message(host_id, pane_id, if on { "/remote-control" } else { "/remote-control off" }, false, None, false, false).await,
        Some(AgentKind::Claude) => {}
        _ => return Err(http(400, "Remote Control works with Claude Code and Antigravity")),
    }
    send_message(host_id, pane_id, "/remote-control", false, None, false, false).await?;
    for i in 0..12 {
        sleep(300).await;
        let s = current_screen(host_id, pane_id).await?;
        let wanted = if on { re!(r"^continue", "i") } else { re!(r"disconnect", "i") };
        if let Some(opt) = s.prompt.as_ref().and_then(|p| p.options.iter().find(|o| wanted.is_match(&o.label))) {
            if on {
                // already on: just close the menu
                keys(host_id, pane_id, &["Escape".to_owned()]).await?;
            } else {
                answer(host_id, pane_id, opt.n as u32, None, None).await?;
            }
            return Ok(());
        }
        // turning on shows no menu
        if s.prompt.is_none() && i > 3 {
            return Ok(());
        }
    }
    Ok(())
}

/// Claude Code's ultracode (a dynamic workflow for every task): `/effort ultracode on|off`, for
/// this session only. The box's rule shows the switch once Claude has taken it; when it can't,
/// Claude says why under the command, and that is the error the app shows.
pub async fn set_ultracode(host_id: &str, pane_id: &str, on: bool) -> HubResult<()> {
    let (host, found) = agent_pane(host_id, pane_id)?;
    if found.pane.agent.as_ref().map(|a| a.kind) != Some(AgentKind::Claude) {
        return Err(http(400, "Ultracode is a Claude Code setting"));
    }
    let before = current_screen(host_id, pane_id).await?;
    if before.prompt.is_some() {
        return Err(http(409, "Answer the open question first"));
    }
    if before.ultracode == Some(on) {
        return Ok(());
    }
    send_message_to_pane(host_id, pane_id, &format!("/effort ultracode {}", if on { "on" } else { "off" }), false, false).await?;
    let mut t = 0;
    while t < 3000 {
        sleep(150).await;
        let lines = capture_panes(&host, &[pane_id.to_owned()]).await?.get(pane_id).cloned().unwrap_or_default();
        let s = parse_screen(&lines, width_of(&found.pane), AgentKind::Claude);
        if s.ultracode == Some(on) {
            return Ok(());
        }
        // Claude's answer to `/effort ultracode` when it can't: dynamic workflows are off in its /config, or the model has none
        let refused = re!(r"⎿\s+(Ultracode (?:needs|isn't|is not|keyword)[^\n]*?)\s*$");
        if let Some(why) = lines.iter().find_map(|l| refused.captures(l).map(|c| c[1].to_owned()).filter(|w| !w.is_empty())) {
            return Err(http(409, re!(r"\s+", "g").replace_all(&why, " ").jtrim().to_owned()));
        }
        t += 150;
    }
    // no word from Claude yet (a slow redraw): the next screen read says where it stands
    nudge(host_id, pane_id);
    Ok(())
}

/// Claude writes background task output to /tmp/claude-<uid>/<project>/<session>/tasks/<id>.output.
pub async fn find_task_output(host_id: &str, id: &str) -> HubResult<Option<String>> {
    if !re!(r"^[\w-]{4,40}$").is_match(id) {
        return Err(http(400, "Bad task id"));
    }
    let r = registry().host(host_id)?.run(&format!("find /tmp -maxdepth 6 -path {} 2>/dev/null | head -1", shq(&format!("*/tasks/{id}.output")))).await?;
    let out = r.stdout.jtrim();
    Ok((!out.is_empty()).then(|| out.to_owned()))
}

/// Names a session. Claude, Codex and agy keep the name themselves (/rename), so it shows the same
/// everywhere: in this hub, the phone app and their own resume pickers. While the agent is busy the
/// name waits and goes in as soon as it's idle. Other agents keep the name in Inyon against their
/// conversation; plain terminals get their tmux window renamed. An empty name goes back to the
/// automatic one.
pub async fn rename_pane(host_id: &str, pane_id: &str, raw: &str) -> HubResult<()> {
    let spaced = re!(r"\s+", "g").replace_all(raw, " ");
    let name = spaced.jtrim().to16(80).to_owned();
    let (host, found) = agent_pane(host_id, pane_id)?;
    let Some(a) = found.pane.agent.clone() else {
        let cmd = if name.is_empty() {
            format!("tmux set-window-option -t {} automatic-rename on", shq(pane_id))
        } else {
            format!("tmux rename-window -t {} {}", shq(pane_id), shq(&tmux_literal(&name, on_psmux(host_id))))
        };
        let r = host.run(&cmd).await?;
        if r.code != 0 {
            let err = r.stderr.jtrim();
            return Err(http(500, if err.is_empty() { "Could not rename the window" } else { err }));
        }
        registry().refresh(host_id, 0);
        return Ok(());
    };
    let Some(session) = a.session_id.clone().filter(|s| !s.is_empty()) else {
        return Err(http(409, "Send a first message, then name the session"));
    };
    // the title the agent made up, so the automatic name can put it back
    let made = if owns_title(Some(a.kind)) {
        let title = watchers().peek(host_id, pane_id).and_then(|w| w.info().title).unwrap_or_else(|| found.pane.title.clone());
        Some(bare_title(Some(&title))).filter(|t| !t.is_empty())
    } else {
        None
    };
    let was = original_title(host_id, &session).or(made);
    set_session_name(host_id, &session, &name, was.as_deref(), Some(a.kind));
    RENAME_TRIED.with(|r| r.borrow_mut().remove(&format!("{host_id}:{pane_id}")));
    apply_pending_name(host_id, pane_id).await?;
    registry().refresh(host_id, 0);
    Ok(())
}

/// Renames a tmux session (psmux on Windows too): `session` is its number, the `52` of `$52`.
/// tmux won't have a dot or a colon in a session's name, so those become underscores, as tmux
/// itself does; a # (a tmux format) becomes a dash, as when Inyon starts a session.
pub async fn rename_tmux_session(host_id: &str, session: &str, raw: &str) -> HubResult<String> {
    if !re!(r"^\d{1,9}$").is_match(session) {
        return Err(http(400, "Bad session"));
    }
    let spaced = re!(r"\s+", "g").replace_all(raw, " ");
    let name = re!(r"[.:]", "g").replace_all(spaced.jtrim(), "_").replace('#', "-").to16(80).to_owned();
    if name.is_empty() {
        return Err(http(400, "Type a name"));
    }
    let host = registry().host(host_id)?;
    let r = host.run(&format!("tmux rename-session -t {} -- {}", shq(&format!("${session}")), shq(&name))).await?;
    if r.code != 0 {
        let err = r.stderr.jtrim();
        if re!(r"duplicate session", "i").is_match(err) {
            return Err(http(409, format!("There's already a tmux session called {name} on that machine")));
        }
        return Err(http(500, if err.is_empty() { "Could not rename the tmux session" } else { err }));
    }
    registry().refresh(host_id, 0);
    Ok(name)
}

/// Types /rename for a name the agent hasn't taken yet, once it's idle with nothing open on screen.
/// Called after a rename and again by the registry while one waits; tried at most every 20 s.
pub async fn apply_pending_name(host_id: &str, pane_id: &str) -> HubResult<()> {
    let Some(a) = registry().find_pane(host_id, pane_id).and_then(|f| f.pane.agent) else { return Ok(()) };
    let Some(want) = pending_name(host_id, a.session_id.as_deref()) else { return Ok(()) };
    if !owns_title(Some(a.kind)) || a.state != AgentState::Idle {
        return Ok(());
    }
    let key = format!("{host_id}:{pane_id}");
    let tried = RENAME_TRIED.with(|r| r.borrow().get(&key).copied()).unwrap_or(0);
    if now_ms().saturating_sub(tried) < 20_000 {
        return Ok(());
    }
    RENAME_TRIED.with(|r| r.borrow_mut().insert(key, now_ms()));
    let s = current_screen(host_id, pane_id).await?;
    // leave a half-typed message alone (Codex's is set aside and put back); try again later
    if s.prompt.is_some() || s.working || (nonempty(&s.draft) && a.kind != AgentKind::Codex) {
        return Ok(());
    }
    let host = registry().host(host_id)?;
    let line = format!("/rename {}", want.name);
    if a.kind == AgentKind::Codex {
        keeping_draft(host_id, pane_id, || codex_command(host_id, pane_id, &line)).await
    } else {
        type_into(&host, pane_id, &line, true).await
    }
}

/// The registry's hook (`registry.on` at import in the TS): a name waiting for an idle agent.
pub(crate) fn on_registry_event(e: &RegistryEvent) {
    if let RegistryEvent::RenameDue { host, pane } = e {
        let (host, pane) = (host.clone(), pane.clone());
        spawn(async move {
            let _ = apply_pending_name(&host, &pane).await;
        });
    }
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
