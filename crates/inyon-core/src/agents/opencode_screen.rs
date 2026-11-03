//! Port of agents/opencode-screen.ts.

use inyon_protocol::{Num, PromptOption, PromptTab, ScreenPrompt, ScreenState};

use crate::claude::screen::{new_state, opt, other_escape, prompt, sgr_codes};
use crate::js::fmt_number;
use crate::js::re::split_plain;
use crate::js::text::JsStr;
use crate::re;

/// a row of OpenCode's input box (or the permission/question panel that replaces it): the bar sits at column 2
macro_rules! BOX {
    () => {
        re!(r"^\s{0,3}┃")
    };
}
/// what /undo leaves in the box ("1 message reverted", "ctrl+x r or /redo to restore"): not typed text
macro_rules! NOTICE {
    () => {
        re!(r"^(\d+ messages? reverted|.*\/redo to restore)$")
    };
}
/// "1. Tabs", or "2. [✓] Green" in a multi-select question
macro_rules! OPTION {
    () => {
        re!(r"^(\d+)\.\s+(?:\[([ ✓✔x×])\]\s+)?(.+)$")
    };
}
/// the hint row under a question: "↑↓ select  enter submit  esc dismiss" (toggle for multi-select, confirm in a multi-question dialog)
macro_rules! QUESTION_FOOTER {
    () => {
        re!(r"enter (submit|confirm|toggle)|esc dismiss")
    };
}
/// "15.8K (8%)" in the status row, which wraps on narrow panes ("16.1K (8%ctrl+p")
macro_rules! STATUS_PCT {
    () => {
        re!(r"\((\d{1,3})%")
    };
}
macro_rules! SIDE_TOKENS {
    () => {
        re!(r"^([\d,]+) tokens$")
    };
}
macro_rules! SIDE_PCT {
    () => {
        re!(r"^(\d{1,3})% used$")
    };
}
macro_rules! SIDE_COST {
    () => {
        re!(r"^\$(\d+(?:\.\d+)?) spent$")
    };
}

struct Row {
    at: usize,
    text: String,
}

/// OpenCode 1.19 TUI. The input box is a column of "┃" rows ending in a footer row
/// ("Build · Big Pickle OpenCode · high": agent, model with its provider, variant) above a "╹▀▀▀"
/// edge, with a status row under it ("⬝⬝⬝ esc interrupt … 16.6K (8%)  ctrl+p commands"). A
/// permission request or a question from the agent takes the box's place. Wide windows add a
/// sidebar on the right (title, context tokens, % used, $ spent), which is cut off the rows and
/// read on its own. `styled` is the same screen with its colours, used to tell which tab of a
/// multi-question dialog is open.
pub fn parse_opencode_screen(lines: &[String], width: Option<u32>, styled: Option<&[String]>) -> ScreenState {
    let width = width.unwrap_or(120);
    let mut state = new_state(width);
    let mut all: Vec<String> = lines.iter().map(|l| l.jtrim_end().to_owned()).collect();
    while all.last().is_some_and(|l| l.is_empty()) {
        all.pop();
    }
    let w = if width > 0 { width as usize } else { all.iter().map(|l| l.len16()).max().unwrap_or(0) };
    // the sidebar starts ~40 columns from the right edge, after a wide gap (the TS looks ahead for
    // the next character; here it is matched and left out of the gap)
    let split = |l: &str| -> (String, Option<String>) {
        for m in re!(r"(\s{4,})\S", "g").captures_iter(l) {
            let gap = m.get(1).unwrap();
            let at = l.idx16(gap.end());
            if w >= 100 && at as i64 >= w as i64 - 46 {
                return (l[..gap.start()].to_owned(), Some(l[gap.end()..].jtrim().to_owned()));
            }
        }
        (l.to_owned(), None)
    };
    for l in &all {
        let Some(side) = split(l).1 else { continue };
        if side.is_empty() {
            continue;
        }
        if let Some(tokens) = SIDE_TOKENS!().captures(&side) {
            state.context_used = Some(Num(crate::js::to_number(&tokens[1].replace(',', ""))));
        }
        if let Some(pct) = SIDE_PCT!().captures(&side) {
            state.context_pct = Some(Num(crate::js::to_number(&pct[1])));
        }
        if let Some(cost) = SIDE_COST!().captures(&side) {
            state.cost_usd = Some(Num(crate::js::to_number(&cost[1])));
        }
    }

    // the box ends in a "╹▀▀▀" edge; on the home screen (no conversation yet) it's centred, so its
    // column is read off that edge rather than assumed
    let edge = (0..all.len()).rev().find(|&i| all[i].contains("╹▀▀"));
    let col: isize = edge.map_or(-1, |e| all[e].index_of16("╹", 0).map_or(-1, |c| c as isize));
    let below: &[String] = match edge {
        Some(e) => &all[e + 1..],
        None => &all[all.len().saturating_sub(3)..],
    };
    let status = below.iter().find(|l| re!(r"esc interrupt|esc again to interrupt|ctrl\+p\b").is_match(l));
    if let Some(status) = status.filter(|s| re!(r"esc (again to )?interrupt").is_match(s)) {
        state.working = true;
        let stripped = re!(r"^[\s⬝■]+").replacen(status, 1, "");
        let text = split_plain(&stripped, re!(r"\s{2,}"))[0].to_owned();
        state.activity = Some(if re!(r"retrying", "i").is_match(status) {
            match re!(r"retrying in \d+s", "i").find(&text) {
                Some(m) => format!("Retrying ({})", re!(r"^retrying ", "i").replacen(m.as_str(), 1, "")),
                None => "Retrying".into(),
            }
        } else {
            "Working".into()
        });
    }
    // the status row's percentage is the context window; its token count isn't (the sidebar's is).
    // Nothing under the edge is sidebar, however far right it sits.
    if state.context_pct.is_none() {
        if let Some(pct) = below.iter().find_map(|l| STATUS_PCT!().captures(l)) {
            state.context_pct = Some(Num(crate::js::to_number(&pct[1])));
        }
    }

    let in_box = |l: &str| -> bool {
        if col > 3 {
            l.at16(col as usize) == Some('┃') && l.to16(col).is_blank()
        } else {
            BOX!().is_match(l)
        }
    };
    let end: Option<usize> = if col > 3 { edge.and_then(|e| e.checked_sub(1)) } else { (0..all.len()).rev().find(|&i| BOX!().is_match(&all[i])) };
    let Some(end) = end else { return state };
    if !in_box(&all[end]) {
        return state;
    }
    let mut start = end;
    while start > 0 && in_box(&all[start - 1]) && end - start < 40 {
        start -= 1;
    }
    // each row with the line it came from, so its colours can be looked up
    let rows: Vec<Row> = all[start..=end]
        .iter()
        .enumerate()
        .map(|(i, l)| Row { at: start + i, text: l.clone() })
        // the slash-command popup is drawn inside its own ┃…┃ frame right above the box (filtered
        // before the sidebar cut, which would take its right edge)
        .filter(|r| !re!(r"┃.*\S.*┃\s*$").is_match(&r.text))
        .map(|r| {
            let cut = if col > 3 { r.text.from16(col).to_owned() } else { split(&r.text).0 };
            let t = re!(r"^\s{0,3}┃ {0,2}").replacen(&cut, 1, "");
            Row { at: r.at, text: t.jtrim_end().to_owned() }
        })
        .collect();
    let texts: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();

    // a menu opened from the TUI (models, variants, agents, sessions) sits over everything with "esc" in its title row
    // (transcript text can show to its left, and the sidebar to its right)
    let title = all[..start].iter().filter_map(|l| re!(r"(?:^|\s{3,})([A-Z][\w ]{2,40}?) {3,}esc(?: {2,}|$)").captures(l).map(|c| c[1].to_owned())).find(|t| !t.is_empty());
    let dialog = title.is_some() || all[..start].iter().any(|l| re!(r"\S {3,}esc(?: {2,}|$)").is_match(l));
    // on the home screen the menu can cover the footer too
    if dialog {
        state.input_visible = Some(false);
        if let Some(t) = &title {
            state.dialog = Some(t.jtrim().to_owned());
        }
    }

    // "Allow always" asks once more ("Always allow … until OpenCode is restarted", Confirm / Cancel)
    if let Some(permission) = texts.iter().position(|r| re!(r"Permission required|△ Always allow\s*$").is_match(r)) {
        let always = re!(r"Always allow\s*$").is_match(texts[permission]);
        let option_row = (permission + 1..texts.len()).find(|&i| if always { re!(r"Confirm\s{2,}Cancel").is_match(texts[i]) } else { texts[i].contains("Allow once") });
        let source = match option_row {
            Some(r) => texts[r],
            None if always => "Confirm   Cancel",
            None => "Allow once   Allow always   Reject",
        };
        let labels: Vec<String> = split_plain(source, re!(r"\s{2,}"))
            .into_iter()
            .map(|s| s.jtrim().to_owned())
            .filter(|s| !s.is_empty() && !re!(r"fullscreen|select|confirm|ctrl\+").is_match(s))
            .collect();
        let body: Vec<String> =
            texts[permission + 1..option_row.unwrap_or(texts.len())].iter().map(|r| re!(r"^[\s←→↳]+").replacen(r, 1, "").jtrim().to_owned()).filter(|s| !s.is_empty()).collect();
        let head = if always { "Always allow this until OpenCode restarts?" } else { "OpenCode needs permission" };
        let mut p = prompt(std::iter::once(head.to_owned()).chain(body).collect(), labels.into_iter().enumerate().map(|(i, l)| opt(i as u64 + 1, l, i == 0)).collect());
        p.navigate = Some(true);
        state.prompt = Some(p);
        return state;
    }

    let footer_row = |r: &str| re!(r"^\S.* · \S").is_match(r.jtrim());
    if let Some(footer_at) = texts.iter().position(|r| QUESTION_FOOTER!().is_match(r)) {
        if !texts[..footer_at].iter().any(|r| footer_row(r)) {
            let mut panel: &[Row] = &rows[..footer_at];
            while panel.first().is_some_and(|r| r.text.is_blank()) {
                panel = &panel[1..];
            }
            // a dialog with several questions (or one with checkboxes) opens with a tab strip:
            // "Indentation   Colours   Confirm", the open tab drawn on its own background
            let first: Vec<String> = panel.first().map(|r| split_plain(r.text.jtrim(), re!(r"\s{2,}")).into_iter().map(str::to_owned).collect()).unwrap_or_default();
            let mut tabs: Option<Vec<PromptTab>> = None;
            if first.len() >= 2 && first.last().map(String::as_str) == Some("Confirm") {
                let current = current_tab(styled.and_then(|s| s.get(panel[0].at)).map(String::as_str), &first);
                tabs = Some(
                    first
                        .iter()
                        .enumerate()
                        .map(|(i, label)| PromptTab { label: label.clone(), done: current >= 0 && (i as isize) < current, current: i as isize == current })
                        .collect(),
                );
                panel = &panel[1..];
            }
            let body: Vec<&str> = panel.iter().map(|r| r.text.as_str()).collect();
            let questions = tabs.as_ref().map_or(1, |t| t.len()) as isize - if tabs.is_some() { 1 } else { 0 };
            let current = tabs.as_ref().and_then(|t| t.iter().position(|t| t.current)).map_or(-1, |c| c as isize);
            let last = if current < 0 { questions <= 1 } else { current >= questions - 1 };
            let first_option = body.iter().position(|r| OPTION!().is_match(r));
            // the Confirm tab: what was answered, and Enter sends it all
            if first_option.is_none() && re!(r"^Review$", "i").is_match(body.iter().find(|r| !r.is_blank()).copied().unwrap_or("")) {
                let mut p = prompt(body.iter().map(|r| r.jtrim().to_owned()).filter(|s| !s.is_empty()).collect(), vec![]);
                p.confirm = Some("Submit".into());
                p.tabs = tabs;
                state.prompt = Some(p);
                return state;
            }
            if let Some(first_option) = first_option {
                let mut options: Vec<PromptOption> = Vec::new();
                let mut multi = false;
                for r in &body[first_option..] {
                    if let Some(m) = OPTION!().captures(r) {
                        // Number(m[1]): a long run of digits is still its number
                        let mut option = opt(crate::js::to_number(&m[1]) as u64, m[3].jtrim(), options.is_empty());
                        if let Some(c) = m.get(2) {
                            option.checked = Some(c.as_str() != " ");
                            multi = true;
                        }
                        options.push(option);
                    } else if !r.is_blank() && !options.is_empty() {
                        let prev = options.last_mut().unwrap();
                        prev.detail = Some(match prev.detail.take().filter(|d| !d.is_empty()) {
                            Some(d) => format!("{d} {}", r.jtrim()),
                            None => r.jtrim().to_owned(),
                        });
                    }
                }
                let free = options.iter().find(|o| re!(r"^Type your own answer$", "i").is_match(&o.label)).map(|o| o.n);
                let mut p: ScreenPrompt = prompt(body[..first_option].iter().map(|r| r.jtrim().to_owned()).filter(|s| !s.is_empty()).collect(), options);
                p.multi = multi;
                p.free_text_option = free;
                p.tabs = tabs;
                // checkboxes are toggled one by one; Tab moves on to the next question or the Confirm step
                if multi {
                    p.confirm = Some(if last { "Confirm" } else { "Next" }.into());
                }
                state.prompt = Some(p);
                return state;
            }
        }
    }

    // the input box: text rows, then the agent · model footer
    let Some(footer) = (0..texts.len()).rev().find(|&i| footer_row(texts[i])) else { return state };
    state.input_visible = Some(!dialog);
    let mut parts: Vec<&str> = texts[footer].jtrim().split(" · ").collect();
    state.mode = Some(parts[0].jtrim().to_lowercase());
    // a third part is the model's variant, whatever it's called (low, max, a name from the config)
    let variant = if parts.len() > 2 && re!(r"^[\w.-]+$").is_match(parts[parts.len() - 1].jtrim()) { parts.pop().map(|p| p.jtrim().to_lowercase()) } else { None };
    let model = parts[1..].join(" · ");
    let model = model.jtrim();
    state.model = (!model.is_empty()).then(|| model.to_owned());
    state.effort = variant;
    let draft = texts[..footer].iter().filter(|r| !NOTICE!().is_match(r.jtrim())).copied().collect::<Vec<_>>().join("\n");
    let draft = re!(r"^\n+|\n+$", "g").replace_all(&draft, "");
    // the dimmed placeholder isn't typed text
    let d = draft.jtrim();
    if !d.is_empty() && !d.starts_with("Ask anything…") {
        state.draft = Some(d.to_owned());
    }
    state
}

/// Which tab of a question dialog is open: the one drawn on a background of its own (the theme's
/// accent), where the others share the box's. -1 when the row's colours aren't there to read.
pub fn current_tab(styled_row: Option<&str>, labels: &[String]) -> isize {
    let Some(row) = styled_row else { return -1 };
    let mut bg = String::new();
    let mut text = String::new();
    // the background under each UTF-16 unit of `text`
    let mut under: Vec<String> = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let rest = &row[i..];
        let ch = rest.chars().next().unwrap();
        if ch == '\x1b' {
            if let Some(sgr) = re!(r"^\x1b\[([\d;:]*)m").captures(rest) {
                let codes = sgr_codes(&sgr[1]);
                let mut k = 0;
                while k < codes.len() {
                    let c = codes[k];
                    if c == 0.0 || c == 49.0 {
                        bg.clear();
                    } else if c == 48.0 {
                        let next = codes.get(k + 1).copied();
                        let n = if next == Some(2.0) {
                            4
                        } else if next == Some(5.0) {
                            2
                        } else {
                            0
                        };
                        bg = codes[k..(k + n + 1).min(codes.len())].iter().map(|x| fmt_number(*x)).collect::<Vec<_>>().join(";");
                        k += n;
                    } else if (40.0..=47.0).contains(&c) || (100.0..=107.0).contains(&c) {
                        bg = fmt_number(c);
                    } else if c == 38.0 || c == 58.0 {
                        let next = codes.get(k + 1).copied();
                        k += if next == Some(5.0) {
                            2
                        } else if next == Some(2.0) {
                            4
                        } else {
                            0
                        };
                    }
                    k += 1;
                }
                i += sgr[0].len();
                continue;
            }
            i += other_escape(rest);
            continue;
        }
        text.push(ch);
        for _ in 0..ch.len_utf16() {
            under.push(bg.clone());
        }
        i += ch.len_utf8();
    }
    // the box's own background is the one under its bar
    let bar: isize = text.index_of16("┃", 0).map_or(-1, |b| b as isize);
    let base_at: isize = if bar >= 0 { bar } else { text.find(|c: char| !crate::js::text::is_space(c)).map_or(-1, |b| text.idx16(b) as isize) };
    let base = if base_at >= 0 { under.get(base_at as usize).cloned().unwrap_or_default() } else { String::new() };
    let mut from = (bar + 1) as usize;
    for (i, label) in labels.iter().enumerate() {
        let Some(at) = text.index_of16(label, from) else { return -1 };
        if under.get(at).cloned().unwrap_or_default() != base {
            return i as isize;
        }
        from = at + label.len16();
    }
    -1
}

#[cfg(test)]
#[path = "opencode_screen_tests.rs"]
mod tests;
