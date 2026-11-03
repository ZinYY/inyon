//! Port of claude/drafts.ts: the unsent message for each pane, shared by every device and the
//! terminal's input box, kept in the store under "drafts".

use std::cell::{Cell, RefCell};

use indexmap::IndexMap;
pub use inyon_protocol::DraftSource;
use serde::{Deserialize, Serialize};

use crate::hosts::http_error::HubResult;
use crate::hosts::registry::registry;
pub use crate::js::text::squash;
use crate::js::JsStr;
use crate::platform::rt::{self, set_timeout, Timer};
use crate::platform::store::kv;
use crate::tmux::tmux::{send_keys, type_into};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub source: DraftSource,
    pub at: f64,
    /// what we last put into (or read from) the terminal's input box
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrored: Option<String>,
}

/// What `setDraft` keeps: a box emptied in the app is still a draft until the terminal's box is
/// empty too: without what it holds, a screen read in between would take the old text for typing
/// there and bring it back.
pub fn keeps_draft(d: Option<&Draft>) -> bool {
    d.is_some_and(|d| !d.text.is_empty() || d.mirrored.as_deref().is_some_and(|m| !m.is_empty()))
}

/// Rows a draft fills in the input box. The screen reader joins wrapped rows back into one line,
/// but Ctrl+U only clears the row the cursor is on, so a long dictated paragraph needs one per row.
pub fn draft_rows(draft: &str, width: Option<u32>) -> usize {
    let row = (width.filter(|w| *w != 0).unwrap_or(80) as i64 - 6).max(20) as usize;
    draft.split('\n').map(|line| line.len16().div_ceil(row).max(1)).sum::<usize>() + 2
}

/// The keys `clearInput` sends to empty Claude's input box: Ctrl+U clears a row, Backspace joins
/// the row above.
pub fn clear_input_keys(draft: &str, width: Option<u32>) -> Vec<&'static str> {
    let rows = draft_rows(draft, width);
    let mut keys = vec![];
    for _ in 0..(rows + 1).min(240) {
        keys.extend(["C-u", "BSpace"]);
    }
    keys.push("C-u");
    keys
}

/// What `mirrorToTerminal` does to put `text` into the input box (replacing `current`): None when
/// the box already holds it, else the keys that clear it first (if anything is there) and the
/// text to type (if any).
pub fn mirror_steps(text: &str, current: Option<&str>, width: Option<u32>) -> Option<(Option<Vec<&'static str>>, Option<String>)> {
    if squash(current.unwrap_or("")) == squash(text) {
        return None;
    }
    let clear = current.filter(|c| !c.is_empty()).map(|c| clear_input_keys(c, width));
    let typed = (!text.is_empty()).then(|| text.to_owned());
    Some((clear, typed))
}

thread_local! {
    /// read from the store the first time a draft is asked for, as the TS module reads it on load
    static DRAFTS: RefCell<Option<IndexMap<String, Draft>>> = const { RefCell::new(None) };
    static SAVE_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    /// while the phone app is in the background it can be closed at any moment: no waiting then
    static AT_ONCE: Cell<bool> = const { Cell::new(false) };
}

fn with_drafts<R>(f: impl FnOnce(&mut IndexMap<String, Draft>) -> R) -> R {
    DRAFTS.with(|d| {
        let mut d = d.borrow_mut();
        f(d.get_or_insert_with(|| kv::get::<IndexMap<String, Draft>>("drafts").unwrap_or_default()))
    })
}

pub fn save_drafts_at_once(on: bool) {
    AT_ONCE.with(|a| a.set(on));
}

fn save() {
    if AT_ONCE.with(Cell::get) {
        return save_drafts();
    }
    SAVE_TIMER.with(|t| {
        let mut t = t.borrow_mut();
        rt::clear(&mut t);
        *t = Some(set_timeout(500, save_drafts));
    });
}

/// Writes the drafts now instead of after the short wait (the app or hub is about to go).
pub fn save_drafts() {
    SAVE_TIMER.with(|t| rt::clear(&mut t.borrow_mut()));
    let all = with_drafts(|d| d.clone());
    let _ = kv::set("drafts", &all);
}

#[track_caller]
pub fn get_draft(key: &str) -> Option<Draft> {
    crate::platform::rt::assert_core_thread("the drafts");
    with_drafts(|d| d.get(key).cloned())
}

#[track_caller]
pub fn set_draft(key: &str, d: Option<Draft>) {
    crate::platform::rt::assert_core_thread("the drafts");
    with_drafts(|all| match d {
        Some(d) if keeps_draft(Some(&d)) => {
            all.insert(key.to_owned(), d);
        }
        _ => {
            all.shift_remove(key);
        }
    });
    save();
}

/// Clears Claude's input box: Ctrl+U clears a row, Backspace joins the row above.
pub async fn clear_input(host_id: &str, pane_id: &str, draft: &str) -> HubResult<()> {
    let host = registry().host(host_id)?;
    let width = registry().find_pane(host_id, pane_id).and_then(|f| pane_width(&f.pane));
    send_keys(&host, pane_id, &clear_input_keys(draft, width)).await
}

/// Puts `text` into Claude's input box without submitting it (replacing what's there).
pub async fn mirror_to_terminal(host_id: &str, pane_id: &str, text: &str, current: Option<&str>) -> HubResult<()> {
    if squash(current.unwrap_or("")) == squash(text) {
        return Ok(());
    }
    if let Some(c) = current.filter(|c| !c.is_empty()) {
        clear_input(host_id, pane_id, c).await?;
    }
    if !text.is_empty() {
        type_into(&registry().host(host_id)?, pane_id, text, false).await?;
    }
    Ok(())
}

/// A pane's width as `draftRows` takes it (`width || 80`).
fn pane_width(p: &inyon_protocol::PaneInfo) -> Option<u32> {
    let w = *p.width;
    (w.is_finite() && w > 0.0).then_some(w as u32)
}
