//! The resting grid. An agent's window that no person's tmux client shows rests at 200×50, so the
//! agent lays its screen out wide for Inyon's readers, instead of at the size the last phone left
//! it or tmux's 80×24 for a session nobody attached. The window-size option goes back to what it
//! was straight away, so the next client to attach, the laptop or a phone, still sizes the window
//! the way it always does.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::hosts::host::shq;

pub const COLS: u32 = 200;
pub const ROWS: u32 = 50;

/// The windows to rest now: agent windows nobody shows that aren't at the grid's size, skipping
/// one already asked at the size it still has (a tmux too old to resize, or a size someone set).
/// `tried` keeps what was asked, for the windows still unwatched.
pub fn to_rest(unwatched: &IndexMap<String, (u32, u32)>, agent_windows: &HashSet<String>, tried: &mut HashMap<String, (u32, u32)>) -> Vec<String> {
    tried.retain(|w, size| unwatched.get(w) == Some(size));
    let rest: Vec<String> = unwatched.iter().filter(|(w, size)| agent_windows.contains(*w) && **size != (COLS, ROWS) && !tried.contains_key(*w)).map(|(w, _)| w.clone()).collect();
    for w in &rest {
        tried.insert(w.clone(), unwatched[w]);
    }
    rest
}

/// Rests the windows, in one tmux call each so the size and the option change together. Left
/// alone: everything when windows never follow their clients (`window-size manual` for all), and
/// a window someone set a window-size of its own for.
pub fn rest_command(windows: &[String]) -> String {
    let each: Vec<String> = windows
        .iter()
        .map(|w| {
            let w = shq(w);
            format!(
                r#"[ -n "$(tmux show -wv -t {w} window-size 2>/dev/null)" ] || tmux resize-window -t {w} -x {COLS} -y {ROWS} \; set-option -w -t {w} -u window-size 2>/dev/null"#
            )
        })
        .collect();
    format!(r#"[ "$(tmux show -gv window-size 2>/dev/null)" = manual ] || {{ {}; }}; true"#, each.join("; "))
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
