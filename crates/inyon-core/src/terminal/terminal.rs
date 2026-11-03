//! Port of terminal/terminal.ts: the phone's terminal on one pane's window.

use std::rc::Rc;

use super::mirror::{open_mirror, OnOutput};
use crate::hosts::host::{shq, PtyHandle};
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::{registry, RegistryEvent};
use crate::tmux::tmux::VIEW_PREFIX;

/// Mouse bindings for the view's own key table: wheel scrolls into tmux's history, clicks and drags
/// work as usual. Nothing else is bound, so the prefix key does nothing and every other key goes to
/// the program in the pane.
pub const VIEW_KEYS: &str = concat!(
    "tmux bind-key -T inyon-view WheelUpPane if-shell -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' 'send-keys -M' 'copy-mode -e'; ",
    "tmux bind-key -T inyon-view MouseDown1Pane 'select-pane -t = ; send-keys -M'; ",
    "tmux bind-key -T inyon-view MouseDrag1Pane if-shell -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' 'send-keys -M' 'copy-mode -M'",
);

/// The command that opens the view: a throwaway session `view` linking window `window`, showing
/// `pane`. tmux checks the prefix before the key table, so the view turns the prefix off as well;
/// otherwise C-b s or C-b : would still reach other sessions and the command prompt.
pub fn view_command(view: &str, window: &str, pane: &str) -> String {
    let v = shq(view);
    [
        format!("{VIEW_KEYS}; exec tmux -u new-session -s {v} -n inyon-tmp"),
        format!(r"\; link-window -s {} -t {}", shq(window), shq(&format!("{view}:"))),
        format!(r"\; kill-window -t {}", shq(&format!("{view}:inyon-tmp"))),
        format!(r"\; set-option -t {v} status off"),
        format!(r"\; set-option -t {v} key-table inyon-view"),
        format!(r"\; set-option -t {v} prefix None"),
        format!(r"\; set-option -t {v} prefix2 None"),
        format!(r"\; set-option -t {v} destroy-unattached on"),
        format!(r"\; select-pane -t {}", shq(pane)),
    ]
    .join(" ")
}

/// Opens a terminal on one pane's window, and only that window: a throwaway session that holds just
/// a link to it, with no status bar, no prefix and a key table of its own, so the phone can't wander
/// off to other windows or sessions (or switch the laptop's view). The window itself is shared, so
/// what you type shows on the laptop too. The view session dies when we detach.
pub fn open_terminal(host_id: &str, pane_id: &str, cols: u16, rows: u16, on_data: impl FnMut(Vec<u8>) + 'static) -> HubResult<PtyHandle> {
    let found = registry().find_pane(host_id, pane_id).ok_or_else(|| http(404, "Pane not found"))?;
    let host = registry().host(host_id)?;
    let (cols, rows) = (cols.max(10), rows.max(4));
    // psmux runs one server per session, so a view session can't link another session's window
    if registry().get(host_id).is_some_and(|r| r.borrow().state.os.as_deref() == Some("Windows")) {
        let (h, p) = (host_id.to_owned(), pane_id.to_owned());
        let sub: OnOutput = Box::new(move |f: Rc<dyn Fn()>| {
            let off = registry().on(move |e| {
                if let RegistryEvent::PaneOutput { host, pane } = e {
                    if *host == h && *pane == p {
                        f();
                    }
                }
            });
            Box::new(off) as Box<dyn Fn()>
        });
        return Ok(open_mirror(host, pane_id, cols, rows, on_data, Some(sub)));
    }
    let id = crate::ids::random_uuid().replace('-', "");
    let view = format!("{VIEW_PREFIX}{}", &id[..6]);
    host.pty(&view_command(&view, &found.window.id, pane_id), cols, rows, on_data)
}
