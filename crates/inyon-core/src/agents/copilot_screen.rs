//! Port of agents/copilot-screen.ts.

use inyon_protocol::{PromptOption, ScreenState};

use crate::claude::screen::{new_state, opt, prompt};
use crate::js::re::{group, split_plain};
use crate::js::text::JsStr;
use crate::re;

// box edges, including the corners of frames nested inside a dialog (the trust question's folder box)
fn rule(line: &str) -> bool {
    re!(r"^\s*[╭╰└┌]?[─━]{8,}[╮╯┘┐]?\s*$").is_match(line)
}

fn clean(line: &str) -> String {
    let l = re!(r"^(\s*│)+\s?").replacen(line, 1, "");
    let l = re!(r"(\s*│)+\s*$").replacen(&l, 1, "");
    l.jtrim_end().to_owned()
}

/// "No (Esc)" -> "No": the key hint means nothing in the app
fn label(text: &str) -> String {
    re!(r"\s*\((?:esc|enter|tab|y|n|ctrl\+\w)\)\s*$", "i").replacen(text, 1, "").into_owned()
}

/// Copilot 1.0 TUI: a framed ❯ composer and live state in the final status row.
pub fn parse_copilot_screen(lines: &[String], width: Option<u32>) -> ScreenState {
    let mut state = new_state(width.unwrap_or(120));
    let tail: Vec<String> = lines[lines.len().saturating_sub(60)..].iter().map(|line| clean(&re!(r"\x1b\[[0-?]*[ -/]*[@-~]", "g").replace_all(line, ""))).collect();
    let footer = tail
        .iter()
        .rev()
        .find(|line| re!(r"(?:esc interrupt|\/ commands · \? help|\b(?:Manual Approval|Assisted Approval|Allow All)\b)", "i").is_match(line))
        .filter(|f| !f.is_empty())
        .cloned();
    if let Some(footer) = &footer {
        state.working = re!(r"esc interrupt", "i").is_match(footer);
        if state.working {
            let first = split_plain(footer.jtrim(), re!(r"\s{3,}"))[0];
            state.activity = Some(re!(r"^[◎●◉]\s*").replacen(first, 1, "").into_owned());
        }
        if footer.contains("Manual Approval") {
            state.mode = Some("default".into());
        } else if footer.contains("Assisted Approval") {
            state.mode = Some("assisted".into());
        } else if footer.contains("Allow All") {
            state.mode = Some("allow-all".into());
        }
        let parts = split_plain(footer.jtrim(), re!(r"\s{3,}"));
        let model = parts.last().copied().unwrap_or("");
        if !model.is_empty() && model != footer.jtrim() {
            state.model = Some(if re!(r"^Auto(?:\s|$)").is_match(model) { "auto".into() } else { re!(r"\s*\([^)]*\)$").replacen(model, 1, "").into_owned() });
        }
        state.effort = re!(r"\b(none|minimal|low|medium|high|xhigh|max)\b", "i").captures(footer).and_then(|c| group(&c, 1)).map(str::to_lowercase);
    }
    let menu_footer = (0..tail.len()).rev().find(|&i| re!(r"(?:enter to select|esc to cancel)", "i").is_match(&tail[i]) && !tail[i].contains("/ commands"));
    if let Some(menu_footer) = menu_footer {
        let menu = &tail[..menu_footer];
        let mut options: Vec<PromptOption> = Vec::new();
        for line in menu {
            if let Some(numbered) = re!(r"^\s*(❯\s*)?(\d+)\.\s+(.+?)\s*$").captures(line) {
                options.push(opt(crate::js::to_number(&numbered[2]) as u64, label(&numbered[3]), numbered.get(1).is_some()));
            }
        }
        // Model menus are unnumbered and include a search composer. Only expose selectable rows.
        if options.is_empty() && re!(r"Search models|group: recommended|routing profile", "i").is_match(&tail.join("\n")) {
            for line in menu {
                if let Some(m) = re!(r"^\s*(❯\s*)?(Auto|(?:gpt|claude|gemini|o\d)[\w. -]*?)(?:\s+✓)?(?:\s{2,}.*)?\s*$").captures(line) {
                    let mut o = opt(options.len() as u64 + 1, m[2].jtrim(), m.get(1).is_some());
                    o.current = Some(line.contains('✓'));
                    options.push(o);
                }
            }
        }
        let first_option = menu.iter().position(|line| re!(r"❯\s*(?:\d+\.|Auto)").is_match(line));
        let (from, to) = match first_option {
            Some(f) => (f.saturating_sub(8), f),
            // `menu.slice(Math.max(0, -1 - 8), undefined)`: all of it
            None => (0, menu.len()),
        };
        let body: Vec<String> = menu[from..to].iter().map(|s| s.jtrim().to_owned()).filter(|s| !s.is_empty() && !rule(s)).collect();
        let body = body[body.len().saturating_sub(8)..].to_vec();
        let mut p = prompt(body, options);
        p.footer = Some(tail[menu_footer].jtrim().to_owned());
        p.navigate = Some(true);
        state.prompt = Some(p);
        return state;
    }
    let mut i = tail.len() as isize - 1;
    while i > 0 {
        let at = i as usize;
        i -= 1;
        if !re!(r"^\s*[❯!](?:\s|$)").is_match(&tail[at]) || !rule(&tail[at - 1]) {
            continue;
        }
        let end = (at + 1..tail.len()).find(|&n| rule(&tail[n]));
        let Some(end) = end else { continue };
        if footer.is_none() {
            continue;
        }
        state.input_visible = Some(true);
        let shell = re!(r"^\s*!").is_match(&tail[at]);
        let first = re!(r"^\s*[❯!]\s?").replacen(&tail[at], 1, "").into_owned();
        let draft = std::iter::once(first).chain(tail[at + 1..end].iter().cloned()).collect::<Vec<_>>().join("\n");
        let draft = draft.jtrim();
        state.draft = if draft.is_empty() {
            None
        } else {
            let d = if shell { format!("!{draft}") } else { draft.to_owned() };
            let d = re!(r"^(\/plan) \[prompt\]$").replacen(&d, 1, "${1}").into_owned();
            Some(re!(r"^(\/model auto) \[efficiency\|balance\|intelligence\|<tier>\]$").replacen(&d, 1, "${1}").into_owned())
        };
        break;
    }
    // the block-style composer some Copilot builds draw instead: ╻▄▄▄ / ┃ text / ╹▀▀▀
    if state.input_visible != Some(true) && footer.is_some() {
        let bottom = (0..tail.len()).rev().find(|&n| re!(r"^\s*╹▀{8,}").is_match(&tail[n]));
        let top = match bottom {
            Some(b) if b > 0 => (0..b).rev().find(|&n| re!(r"^\s*╻▄{8,}").is_match(&tail[n])),
            _ => None,
        };
        if let (Some(top), Some(bottom)) = (top, bottom) {
            if tail[top + 1..bottom].iter().all(|line| re!(r"^\s*┃").is_match(line)) {
                state.input_visible = Some(true);
                let body: Vec<String> = tail[top + 1..bottom]
                    .iter()
                    .map(|line| {
                        let l = re!(r"^\s*┃\s?").replacen(line, 1, "");
                        re!(r"\s*┃\s*$").replacen(&l, 1, "").into_owned()
                    })
                    .collect();
                let shell = re!(r"^\s*!").is_match(body.first().map(String::as_str).unwrap_or(""));
                let joined = body.join("\n");
                let draft = re!(r"^\s*[❯!]\s?").replacen(&joined, 1, "");
                let draft = draft.jtrim();
                state.draft = if draft.is_empty() { None } else { Some(if shell { format!("!{draft}") } else { draft.to_owned() }) };
            }
        }
    }
    state
}
