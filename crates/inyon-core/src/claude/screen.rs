//! Port of claude/screen.ts: a captured pane (the rows `tmux capture-pane -p -e` printed) read
//! into a [`ScreenState`]. Claude Code is read here; the other agents' screens go to their own
//! readers (Codex, Antigravity and anything generic here too, OpenCode and Copilot in `agents`).

use inyon_protocol::{AgentKind, PromptInput, PromptOption, PromptTab, ScreenPrompt, ScreenState};
use serde::Serialize;

use crate::agents::copilot_screen::parse_copilot_screen;
use crate::agents::opencode_screen::parse_opencode_screen;
use crate::js::js_round;
use crate::js::re::{group, split_plain};
use crate::js::text::{first_word, last_word, JsStr};
use crate::re;

// a session named with /rename shows its name in the rule above the input box: "──── Name ─"
macro_rules! RULE {
    () => {
        re!(r"^\s*─{12,}(?: [^─]{1,100} ─{1,12})?\s*$")
    };
}
/// Dialog tops are drawn with ▔ (sometimes with a hint embedded in the line).
macro_rules! DIALOG_TOP {
    () => {
        re!(r"^\s*▔{12,}")
    };
}
macro_rules! OPTION {
    () => {
        re!(r"^(\s*)(❯\s*)?(\d{1,2})\.\s(.*)$")
    };
}
macro_rules! INPUT {
    () => {
        re!(r"^\s*[❯>!](\s|$)")
    };
}
macro_rules! SPINNER {
    () => {
        re!(r"^\s*([^\s\w])\s+([A-Z][\p{L}'’ -]*…)\s*(?:\((.*)\))?\s*$", "u")
    };
}
macro_rules! MODE {
    () => {
        re!(r"(?:⏵⏵|⏸|⏵)\s+([a-z][a-z ]*?)(?: mode)? on\b")
    };
}
/// Claude Code 2.1.29x labels the input box's top rule "ultracode ─" while ultracode is on
macro_rules! ULTRACODE_RULE {
    () => {
        re!(r"(?:^|\s)ultracode ─{1,12}\s*$")
    };
}
/// its effort line above the box: "◉ xhigh · ultracode · /effort" (older builds drew ● and no ultracode)
macro_rules! EFFORT_LINE {
    () => {
        re!(r"[◉●]\s+(low|medium|high|xhigh|max|auto)(\s+·\s+ultracode)?\s+·\s+\/effort\b")
    };
}
macro_rules! FOOTER {
    () => {
        re!(r"(Esc to|Enter to|to cancel|to navigate|to select|to confirm)", "i")
    };
}
/// "1 shell", "2 monitors", "1 local agent", "1 background dynamic workflow" in the footer:
/// background work owned by the Claude process (2.1.299 names each kind, and "N background tasks"
/// for a mix). "← 1 agent" isn't one of them: it counts the sessions in Claude Code's agents view,
/// the machine's other conversations, so it's dropped before these are read (see AGENTS_VIEW).
macro_rules! BACKGROUND {
    () => {
        re!(
            r"\b\d+ (?:local |background |remote |MCP |Artifact comment )?(?:shells?|monitors?|agents?|tasks?|terminals?|teams?|cloud sessions?|(?:dynamic )?workflows?|bash)\b",
            "g"
        )
    };
}
/// "← 2 agents" in the footer: how many other sessions Claude Code's agents view (← opens it) lists
/// on this machine, a background session from weeks ago included. Not this pane's work: read as
/// such, every Claude pane on a machine with one old background session looked busy for good, and
/// messages held for it never went in.
macro_rules! AGENTS_VIEW {
    () => {
        re!(r"←\s*\d+ agents?\b", "g")
    };
}
/// The agent list under the footer (toggled with ↓): "● main", "◯ general-purpose  Sleep briefly  6s · ↓ 37.9k tokens"
macro_rules! AGENT_ROW {
    () => {
        re!(r"^\s*[●◯]\s")
    };
}
macro_rules! TABS {
    () => {
        re!(r"^\s*←\s+(.+?)\s+→\s*$")
    };
}
macro_rules! CHECKBOX {
    () => {
        re!(r"^\[([ ✔✓x×])\]\s*")
    };
}
/// Working lines of other agents: "⠋ Working (12s • esc to interrupt)" and friends.
macro_rules! INTERRUPTIBLE {
    () => {
        re!(r"(esc|ctrl\+c) to (interrupt|cancel|stop)", "i")
    };
}

fn is_option(l: &str) -> bool {
    OPTION!().is_match(l)
}

/// highlight markers other agents put in front of a numbered option (the TS uses a lookahead
/// for the number; here it is captured and put back)
macro_rules! GENERIC_CURSOR {
    () => {
        re!(r"^(\s*)[›▸➤▶●]\s*(\d{1,2}\.\s)")
    };
}

// ---------- building blocks shared by the screen readers ----------

/// `{ working: false, width }`
pub(crate) fn new_state(width: u32) -> ScreenState {
    ScreenState {
        input_visible: None,
        prompt: None,
        working: false,
        activity: None,
        since: None,
        mode: None,
        draft: None,
        remote: None,
        background: None,
        queue: None,
        questions: None,
        model: None,
        effort: None,
        ultracode: None,
        context_pct: None,
        context_used: None,
        cost_usd: None,
        dialog: None,
        width,
    }
}

/// `{ n, label, selected }`
pub(crate) fn opt(n: u64, label: impl Into<String>, selected: bool) -> PromptOption {
    PromptOption { n, label: label.into(), detail: None, selected, checked: None, current: None }
}

/// `{ body, options, multi: false }`
pub(crate) fn prompt(body: Vec<String>, options: Vec<PromptOption>) -> ScreenPrompt {
    ScreenPrompt {
        body,
        whole_lines: None,
        options,
        tabs: None,
        footer: None,
        multi: false,
        free_text_option: None,
        navigate: None,
        input: None,
        links: None,
        api: None,
        confirm: None,
    }
}

/// `lines.map((l) => l.replace(/\s+$/, ''))`, then the empty rows at the bottom dropped.
fn trimmed_rows(raw: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = raw.iter().map(|l| l.jtrim_end().to_owned()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// `paneWidth || Math.max(0, ...lines.map((l) => l.length))`
pub(crate) fn width_of(pane_width: Option<u32>, lines: &[String]) -> u32 {
    match pane_width {
        Some(w) if w > 0 => w,
        _ => lines.iter().map(|l| l.len16()).max().unwrap_or(0) as u32,
    }
}

/// The input box's rows joined back into what was typed: a row that filled the box was
/// word-wrapped by the agent, not a real newline.
fn join_box(rows: &[String], width: u32, first: &regex::Regex) -> String {
    let mut draft = String::new();
    for (i, l) in rows.iter().enumerate() {
        let text = if i == 0 { first.replacen(l, 1, "").into_owned() } else { l.strip_prefix("  ").unwrap_or(l).to_owned() };
        let prev_full = i > 0 && rows[i - 1].len16() as i64 >= width as i64 - 2;
        if i == 0 {
            draft += &text;
        } else if prev_full {
            draft += " ";
            draft += &text;
        } else {
            draft += "\n";
            draft += &text;
        }
    }
    draft
}

/// A captured line with its styling taken out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unstyled {
    pub text: String,
    pub ghost: bool,
}

/// The numbers of one SGR sequence: "" is [0], and an empty part is 0, as `Number('')` is.
pub(crate) fn sgr_codes(params: &str) -> Vec<f64> {
    if params.is_empty() {
        return vec![0.0];
    }
    params.split([';', ':']).map(crate::js::to_number).collect()
}

/// The length of the escape sequence at the start of `s` (which starts with ESC) that isn't
/// SGR: an OSC link, a cursor move, or ESC and one character.
pub(crate) fn other_escape(s: &str) -> usize {
    re!(r"^\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[\d;?]*[A-Za-z]|.)").find(s).map_or(1, |m| m.len())
}

/// Strips terminal styling from a captured line (tmux capture-pane -e) and notes whether all its
/// text is placeholder: Claude draws its prompt suggestions dimmed, with only the first character
/// inverted as a fake cursor, and only while the input is empty.
pub fn unstyle(line: &str) -> Unstyled {
    if !line.contains('\x1b') {
        return Unstyled { text: line.to_owned(), ghost: false };
    }
    let mut text = String::new();
    let (mut dim, mut inverse) = (false, false);
    let (mut dim_chars, mut solid) = (0, 0);
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let ch = rest.chars().next().unwrap();
        if ch == '\x1b' {
            if let Some(sgr) = re!(r"^\x1b\[([\d;:]*)m").captures(rest) {
                let codes = sgr_codes(&sgr[1]);
                let mut k = 0;
                while k < codes.len() {
                    let c = codes[k];
                    if c == 0.0 {
                        dim = false;
                        inverse = false;
                    } else if c == 2.0 {
                        dim = true;
                    } else if c == 22.0 {
                        dim = false;
                    } else if c == 7.0 {
                        inverse = true;
                    } else if c == 27.0 {
                        inverse = false;
                    } else if c == 38.0 || c == 48.0 || c == 58.0 {
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
            // other escapes (OSC links, cursor moves): skip to their terminator
            i += other_escape(rest);
            continue;
        }
        text.push(ch);
        i += ch.len_utf8();
        if crate::js::text::is_space(ch) || matches!(ch, '❯' | '>' | '›' | '»') {
            continue;
        }
        // JavaScript walks UTF-16 units: an astral character counts twice
        let units = ch.len_utf16();
        if dim {
            dim_chars += units;
        } else if !inverse {
            solid += units;
        }
    }
    Unstyled { text, ghost: dim_chars > 0 && solid == 0 }
}

/// What a Claude pane's footer lists as running in the background: `Some(None)` for nothing,
/// None when the footer isn't visible.
pub fn background_on_screen(lines: &[String]) -> Option<Option<String>> {
    parse_screen(lines, None, AgentKind::Claude).background
}

/// Parses a captured Claude Code screen into what the UI needs.
pub fn parse_screen(captured: &[String], pane_width: Option<u32>, agent: AgentKind) -> ScreenState {
    let styled: Vec<Unstyled> = captured.iter().map(|l| unstyle(l)).collect();
    let raw: Vec<String> = styled.iter().map(|l| l.text.clone()).collect();
    match agent {
        AgentKind::Copilot => return finish(parse_copilot_screen(&raw, pane_width)),
        AgentKind::Opencode => return finish(parse_opencode_screen(&raw, pane_width, Some(captured))),
        AgentKind::Antigravity => return finish(parse_antigravity(&raw, pane_width)),
        AgentKind::Codex => return finish(parse_codex(&raw, &styled.iter().map(|l| l.ghost).collect::<Vec<_>>(), pane_width)),
        AgentKind::Claude => {}
        _ => return finish(parse_generic(&raw, pane_width)),
    }
    let lines = trimmed_rows(&raw);
    let width = width_of(pane_width, &lines);
    let rules: Vec<usize> = (0..lines.len()).filter(|&i| RULE!().is_match(&lines[i])).collect();
    let mut state = new_state(width);

    // Idle/working layout: rule, "❯ input", rule, status lines.
    let mut input_top: Option<usize> = None;
    for k in (0..rules.len().saturating_sub(1)).rev() {
        let top = rules[k];
        let next = lines.get(top + 1).map(String::as_str).unwrap_or("");
        if INPUT!().is_match(next) && !is_option(next) && rules[k + 1] - top <= 15 {
            input_top = Some(top);
            state.input_visible = Some(true);
            // a line that fills the box was word-wrapped by Claude, not a real newline
            let rows = &lines[top + 1..rules[k + 1]];
            let draft = join_box(rows, width, re!(r"^\s*[❯>]\s?"));
            let draft = draft.jtrim();
            // a dimmed suggestion is not something you typed
            if !draft.is_empty() && !styled.get(top + 1).is_some_and(|s| s.ghost) && !draft.starts_with("Try \"") {
                state.draft = Some(draft.to_owned());
            }
            // the switch is on the box itself, so it's known whenever the box is
            state.ultracode = Some(ULTRACODE_RULE!().is_match(&lines[top]));
            let mut i = top as isize - 1;
            while i >= (top as isize - 4).max(0) {
                if let Some(e) = EFFORT_LINE!().captures(&lines[i as usize]) {
                    if state.effort.is_none() {
                        state.effort = Some(e[1].to_owned());
                    }
                    if e.get(2).is_some_and(|g| !g.as_str().is_empty()) {
                        state.ultracode = Some(true);
                    }
                    break;
                }
                i -= 1;
            }
            // every kind of background work in the footer, in order: "1 shell, 1 agent"; an agent's
            // description in the list under it could say anything, so those rows are skipped
            let mut background: Vec<String> = Vec::new();
            for l in &lines[rules[k + 1] + 1..] {
                if AGENT_ROW!().is_match(l) {
                    continue;
                }
                if let Some(m) = MODE!().captures(l) {
                    state.mode = Some(m[1].jtrim().to_owned());
                }
                let l = AGENTS_VIEW!().replace_all(l, "");
                background.extend(BACKGROUND!().find_iter(&l).map(|m| m.as_str().to_owned()));
            }
            // the footer was read: nothing listed means nothing running (a hidden footer says nothing)
            state.background = Some(if background.is_empty() { None } else { Some(background.join(", ")) });
            break;
        }
    }

    if let Some(input_top) = input_top {
        for i in (0..input_top).rev() {
            if let Some(m) = SPINNER!().captures(&lines[i]) {
                state.working = true;
                let g3 = group(&m, 3).filter(|s| !s.is_empty());
                let mut activity = m[2].to_owned();
                if let Some(g3) = g3 {
                    activity += " (";
                    activity += g3;
                    activity += ")";
                }
                state.activity = Some(activity);
                break;
            }
            // Never reuse a spinner from a previous turn in the scrollback.
            if RULE!().is_match(&lines[i]) || INPUT!().is_match(&lines[i]) || re!(r"^\s*●\s").is_match(&lines[i]) {
                break;
            }
        }
        return state;
    }

    let tops: Vec<usize> = (0..lines.len()).filter(|&i| RULE!().is_match(&lines[i]) || DIALOG_TOP!().is_match(&lines[i])).collect();
    state.prompt = parse_prompt(&lines, &tops, width).or_else(|| parse_cursor_menu(&lines)).or_else(|| parse_input_dialog(&lines, &tops, width));
    finish(state)
}

/// Pulls links out of a prompt's text and drops the indent its lines share.
fn finish(mut state: ScreenState) -> ScreenState {
    let width = state.width;
    let Some(prompt) = state.prompt.as_mut() else {
        return state;
    };
    let (body, links) = extract_links(&prompt.body, width);
    let indent = body.iter().filter(|l| !l.is_blank() && *l != "╌").map(|l| l.indent16()).min().unwrap_or(99).min(99);
    prompt.body = body.iter().map(|l| if l == "╌" { l.clone() } else { l.from16(indent.min(l.indent16()) as isize).to_owned() }).collect();
    if !links.is_empty() {
        prompt.links = Some(links);
    }
    state
}

/// Pulls URLs out of dialog text. Claude hard-wraps long URLs at the pane width, so a URL line that
/// fills the width continues on the next line (which starts at column 0).
fn extract_links(body: &[String], width: u32) -> (Vec<String>, Vec<String>) {
    let mut out = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let line = &body[i];
        let Some(m) = re!(r"https?:\/\/\S+").find(line) else {
            out.push(line.clone());
            i += 1;
            continue;
        };
        // a link inside a sentence or command ("$ curl https://x | head") stays part of the text
        if !line[m.end()..].is_blank() {
            out.push(line.clone());
            i += 1;
            continue;
        }
        let mut url = m.as_str().to_owned();
        let mut cur = line.as_str();
        while cur.len16() as i64 >= width as i64 - 1 && i + 1 < body.len() && re!(r"^\S").is_match(&body[i + 1]) && !re!(r"\s").is_match(body[i + 1].jtrim()) {
            i += 1;
            cur = &body[i];
            url += cur.jtrim();
        }
        let url = re!(r"[).,;]+$").replacen(&url, 1, "").into_owned();
        links.push(url);
        let before = line[..m.start()].jtrim();
        if !before.is_empty() {
            out.push(before.to_owned());
        }
        i += 1;
    }
    let mut seen = Vec::new();
    for l in links {
        if !seen.contains(&l) {
            seen.push(l);
        }
    }
    (out, seen)
}

/// Dialogs that ask for text, e.g. /login's "Paste code here if prompted >".
fn parse_input_dialog(lines: &[String], tops: &[usize], _width: u32) -> Option<ScreenPrompt> {
    let tail = lines.len().saturating_sub(14);
    let mut at = None;
    for i in (tail..lines.len()).rev() {
        if re!(r"\S.*\s>\s*$").is_match(&lines[i]) || re!(r"(paste|enter|type)\b.*:\s*$", "i").is_match(&lines[i]) {
            at = Some(i);
            break;
        }
    }
    let at = at?;
    let footer = lines[at + 1..].iter().find(|l| re!(r"esc to (cancel|go back)", "i").is_match(l))?;
    let top: isize = tops.iter().rev().find(|&&t| t < at).map(|&t| t as isize).unwrap_or((at as isize - 16).max(-1));
    let mut body: Vec<String> = lines[(top + 1) as usize..at].iter().map(|l| re!(r"^ {1,3}").replacen(l, 1, "").into_owned()).collect();
    while body.first().is_some_and(|l| l.is_blank()) {
        body.remove(0);
    }
    let label = re!(r"\s*[>:]\s*$").replacen(lines[at].jtrim(), 1, "").into_owned();
    let mut p = prompt(body, vec![]);
    p.footer = Some(footer.jtrim().to_owned());
    p.input = Some(PromptInput { label });
    Some(p)
}

/// `array.findLastIndex(f)`
fn find_last(lines: &[String], f: impl Fn(&str) -> bool) -> Option<usize> {
    (0..lines.len()).rev().find(|&i| f(&lines[i]))
}

/// The last `n` rows (`lines.slice(-n)`).
fn last_n(lines: &[String], n: usize) -> &[String] {
    &lines[lines.len().saturating_sub(n)..]
}

/// Menus without numbers (e.g. /remote-control's "Disconnect this session / Show QR code /
/// Continue"): a ❯ cursor on one item, siblings at the same indent, "Enter to select" below.
fn parse_cursor_menu(lines: &[String]) -> Option<ScreenPrompt> {
    let tail = last_n(lines, 30);
    let footer_at = tail.iter().position(|l| re!(r"Enter to (select|confirm)", "i").is_match(l));
    // some dialogs have no key hint (a held message from another session): then the menu has to be
    // the last thing on screen, where an idle or working Claude has its input box
    let block: &[String] = match footer_at {
        Some(f) => &tail[..f],
        None => &tail[..find_last(tail, |l| !l.is_blank()).map_or(0, |i| i + 1)],
    };
    let cur = find_last(block, |l| re!(r"^\s*❯\s+\S").is_match(l) && !OPTION!().is_match(l))?;
    let col = block[cur].index_of16("❯", 0).unwrap() + 2;
    let is_item = |l: &str| l.len16() > col && l.to16(col as isize).is_blank() && l.at16(col) != Some(' ');
    let mut start = cur;
    while start > 0 && is_item(&block[start - 1]) {
        start -= 1;
    }
    let mut end = cur;
    while end + 1 < block.len() && is_item(&block[end + 1]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    if footer_at.is_none() && block[end + 1..].iter().any(|l| !l.is_blank()) {
        return None;
    }
    let mut options = Vec::new();
    for (i, row) in block.iter().enumerate().take(end + 1).skip(start) {
        let text = if i == cur { row.from16(row.index_of16("❯", 0).unwrap() as isize + 1).jtrim() } else { row.jtrim() };
        // "Deny — drop it and tell the sender it was declined": the button, then what it does
        let parts = split_plain(text, re!(r"\s{3,}| — "));
        let label = parts[0];
        let rest = parts[1..].join(" ");
        let mut o = opt(options.len() as u64 + 1, label, i == cur);
        if !rest.is_empty() {
            o.detail = Some(rest);
        }
        options.push(o);
    }
    // the title and description sit above the items, after the last rule
    let mut top = start as isize - 1;
    while top > 0 && !RULE!().is_match(&block[top as usize]) && !re!(r"^▔+$").is_match(block[top as usize].jtrim()) {
        top -= 1;
    }
    if footer_at.is_none() && top <= 0 {
        return None;
    }
    // a quoted message is framed by dashes and marked "│ "; keep the frame as one '╌' marker
    let body: Vec<String> = block[(top + 1) as usize..start]
        .iter()
        .map(|l| if re!(r"^\s*╌+\s*$").is_match(l) { "╌".to_owned() } else { re!(r"^│ ?").replacen(l.jtrim(), 1, "").into_owned() })
        .filter(|l| !l.is_empty())
        .collect();
    let mut p = prompt(body, options);
    p.footer = footer_at.map(|f| tail[f].jtrim().to_owned());
    p.navigate = Some(true);
    Some(p)
}

fn parse_prompt(lines: &[String], rules: &[usize], width: u32) -> Option<ScreenPrompt> {
    // Box top = last full-width rule that opens the box (its next line isn't an option)
    // and has options somewhere below it.
    let mut top: isize = -1;
    for &r in rules.iter().rev() {
        let below = &lines[r + 1..];
        if !is_option(lines.get(r + 1).map(String::as_str).unwrap_or("")) && below.iter().any(|l| is_option(l)) {
            top = r as isize;
            break;
        }
    }
    // Some prompts have no rule above them (e.g. dialogs at the top of a fresh screen).
    if top < 0 {
        let first = lines.iter().position(|l| is_option(l))?;
        top = (first as isize - 12).max(-1);
    }
    let bx = &lines[(top + 1) as usize..];
    // A question whose options carry previews draws the one for the highlighted option in a box to
    // the right of the labels, with "Notes: press n to add notes" under it. Only the labels are
    // kept, and there a number only moves the highlight: picking takes Enter.
    let preview_at = bx.iter().find_map(|l| re!(r"^\s*(❯\s*)?\d{1,2}\.\s.*?\S\s{3,}[┌│└├╭╰]").find(l).map(|m| l[..m.end()].len16() - 1));
    let without_preview: Vec<String>;
    let bx: &[String] = match preview_at {
        Some(col) => {
            without_preview = bx
                .iter()
                .map(|l| {
                    if l.len16() > col && re!(r"^([┌│└├╭╰]|Notes:)").is_match(l.from16(col as isize)) {
                        l.to16(col as isize).trim_end().to_owned()
                    } else {
                        l.clone()
                    }
                })
                .collect();
            &without_preview
        }
        None => bx,
    };
    let mut options: Vec<PromptOption> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let mut footer: Option<String> = None;
    let mut tabs: Option<Vec<PromptTab>> = None;
    // the option being read, as an index into `options`
    let mut current: Option<usize> = None;
    let mut opt_indent = 0usize;
    let mut prev_len = 0usize;
    let mut prev_text = String::new();

    for l in bx {
        if RULE!().is_match(l) {
            current = None;
            continue;
        }
        // one question has no arrows around its strip: " ☐ Layout"
        let strip = TABS!().captures(l).map(|m| m[1].to_owned()).or_else(|| re!(r"^\s*[☐☒✔]\s+\S").is_match(l).then(|| l.jtrim().to_owned()));
        if let Some(tm) = strip {
            if options.is_empty() {
                tabs =
                    Some(re!(r"([☐☒✔])\s+([^☐☒✔]+)", "g").captures_iter(&tm).map(|m| PromptTab { label: m[2].jtrim().to_owned(), done: &m[1] != "☐", current: false }).collect());
                continue;
            }
        }
        if let Some(om) = OPTION!().captures(l) {
            let mut label = om[4].jtrim().to_owned();
            let mut checked = None;
            if let Some(cb) = CHECKBOX!().captures(&label.clone()) {
                checked = Some(&cb[1] != " ");
                label = label[cb[0].len()..].to_owned();
            }
            let mut o = opt(crate::js::to_number(&om[3]) as u64, label.clone(), om.get(2).is_some());
            o.checked = checked;
            opt_indent = om[1].len16() + group(&om, 2).map_or(0, |s| s.len16());
            options.push(o);
            current = Some(options.len() - 1);
            prev_len = l.len16();
            prev_text = label;
            continue;
        }
        if FOOTER!().is_match(l) && !l.starts_with("  ") {
            footer = Some(l.jtrim().to_owned());
            continue;
        }
        if let Some(ci) = current {
            let t = l.jtrim();
            if t.is_empty() {
                continue;
            }
            let indent = l.indent16();
            if re!(r"^(❯\s*)?(Submit|Next)$").is_match(t) {
                continue;
            }
            if indent > opt_indent {
                // Claude word-wraps labels; only a "word" too long for a whole line (a path) is broken
                // mid-word. Short continuation lines that wouldn't have fit are wrapped label text;
                // anything else is a description line.
                let first = first_word(t);
                let last = last_word(&prev_text);
                let room = width as i64 - indent as i64;
                let cur = &mut options[ci];
                if cur.detail.is_none() && prev_len as i64 >= width as i64 - 1 && (last.len16() + first.len16()) as i64 > room {
                    cur.label += t;
                } else if cur.detail.is_none() && (prev_len + 1 + first.len16()) as i64 > width as i64 {
                    cur.label += " ";
                    cur.label += t;
                } else {
                    cur.detail = Some(match cur.detail.take() {
                        Some(d) => format!("{d} {t}"),
                        None => t.to_owned(),
                    });
                }
                prev_text = t.to_owned();
                prev_len = l.len16();
                continue;
            }
            // text after the options ends the option block
            current = None;
        }
        if options.is_empty() {
            body.push(l.strip_prefix(' ').unwrap_or(l).to_owned());
        }
    }

    if options.is_empty() {
        return None;
    }
    // drop separator-only lines at the edges
    while body.first().is_some_and(|l| l.is_blank()) {
        body.remove(0);
    }
    while body.last().is_some_and(|l| l.is_blank()) {
        body.pop();
    }

    let multi = options.iter().any(|o| o.checked.is_some());
    let free = options.iter().find(|o| re!(r"^Type something\.?$", "i").is_match(&o.label)).map(|o| o.n);
    if let Some(t) = tabs.as_mut().filter(|t| !t.is_empty()) {
        // The strip doesn't mark the current tab in plain text; infer it. The last tab is "Submit".
        let last = t.len() - 1;
        let reviewing = body.iter().any(|l| re!(r"Review your answers|Ready to submit", "i").is_match(l));
        let first_open = t[..last].iter().position(|t| !t.done);
        let idx = if reviewing {
            last
        } else {
            match first_open {
                None => last.saturating_sub(1),
                Some(f) => f,
            }
        };
        // the Submit tab always shows ✔; it is only "done" once you're on it
        for (i, tab) in t.iter_mut().enumerate() {
            tab.current = i == idx;
            if i == last {
                tab.done = false;
            }
        }
    }
    let mut p = prompt(tidy_body(&body, width), options);
    p.whole_lines = Some(true);
    p.tabs = tabs;
    p.footer = footer;
    p.multi = multi;
    p.free_text_option = free;
    if preview_at.is_some() {
        p.navigate = Some(true);
    }
    Some(p)
}

/// One line for a prompt: the alert's text and the Needs-you card. A command prompt reads as what
/// the command is for, then its first line ("Re-run the test suite: npm test"); other framed
/// prompts as their title and the first framed line; the rest as their question.
pub fn prompt_summary(prompt: &ScreenPrompt) -> String {
    let raw: Vec<&str> = prompt.body.iter().map(|l| l.jtrim()).collect();
    let sep = raw.iter().position(|l| *l == "╌");
    // the framed block (between ╌ separators) is the command or file being asked about
    let framed = sep.and_then(|s| raw[s + 1..].iter().find(|l| !l.is_empty() && **l != "╌").copied());
    let keep = |l: &str| !l.is_empty() && l != "╌" && !l.starts_with("Tip:");
    let body: Vec<&str> = raw.iter().copied().filter(|l| keep(l)).collect();
    let question = body.iter().find(|l| l.ends_with('?')).copied();
    let title = body.first().copied();
    let cut = |s: String| s.to16(200).to_owned();
    // Codex's approvals: the command ("$ …") or the file ("Destination:" and the path under it,
    // which may be wrapped) is the useful part, not the question around it
    if let Some(q) = question {
        if re!(r"run the following command\?$", "i").is_match(q) {
            if let Some(cmd) = body.iter().find(|l| re!(r"^\$ \S").is_match(l)) {
                return cut(format!("Run {}", cmd.from16(2)));
            }
        }
        if re!(r"make the following edits\?$", "i").is_match(q) {
            let at = body.iter().position(|l| *l == "Destination:");
            let path: String = match at {
                Some(a) => body[a + 1..].iter().filter(|l| !re!(r"^[A-Z][a-z]+:").is_match(l)).copied().collect(),
                None => String::new(),
            };
            let file = path.split('/').next_back().unwrap_or("");
            if !file.is_empty() {
                return format!("Edit {file}?");
            }
        }
    }
    if let (Some(framed), Some(title)) = (framed, title) {
        let sep = sep.unwrap();
        let from = raw.iter().position(|l| *l == title).map_or(0, |i| i + 1);
        let about: Vec<&str> = if from < sep { raw[from..sep].iter().copied().filter(|l| keep(l)).collect() } else { vec![] };
        let about = about.join(" ");
        // "Bash command" / "PowerShell command", then a line saying what it's for
        if re!(r"command$", "i").is_match(title) {
            let what = if about.is_empty() { title } else { &about };
            return cut(format!("{}: {framed}", re!(r"[?:.]$").replacen(what, 1, "")));
        }
        // "Edit file" / "Create file": the file's name sits above the frame, which holds the diff
        if re!(r"file$", "i").is_match(title) && !about.is_empty() {
            return cut(format!("{title}: {about}"));
        }
        return cut(format!("{}: {framed}", re!(r"[?:.]$").replacen(title, 1, "")));
    }
    if let Some(input) = &prompt.input {
        return input.label.clone();
    }
    question.or(title).unwrap_or("Needs your input").to_owned()
}

/// Whether `prompt_summary`'s one line holds everything an answer agrees to. Quick answers (the
/// overview's options, which companion apps and the phone's notifications offer beside that line)
/// are given only then; otherwise the question is answered in the session, where it shows whole. Not whole: a
/// line cut at 200 characters, a framed command or diff of more than one line (a heredoc, a
/// multi-line script), a Codex command that runs on below its `$` line, and Codex's edits, whose
/// changes the line doesn't show.
pub fn prompt_whole(prompt: &ScreenPrompt) -> bool {
    if prompt_summary(prompt).len16() >= 200 {
        return false;
    }
    let raw: Vec<&str> = prompt.body.iter().map(|l| l.jtrim()).collect();
    let body: Vec<&str> = raw.iter().copied().filter(|l| !l.is_empty() && *l != "╌" && !l.starts_with("Tip:")).collect();
    if let Some(q) = body.iter().find(|l| l.ends_with('?')) {
        if re!(r"run the following command\?$", "i").is_match(q) {
            if let Some(i) = body.iter().position(|l| re!(r"^\$ \S").is_match(l)) {
                return body.get(i + 1).is_none_or(|l| re!(r"^[A-Z][a-z]+:").is_match(l) || l.ends_with('?'));
            }
        }
        if re!(r"make the following edits\?$", "i").is_match(q) {
            return false;
        }
    }
    match raw.iter().position(|l| *l == "╌") {
        Some(s) if !body.is_empty() => {
            let end = raw[s + 1..].iter().position(|l| *l == "╌").map_or(raw.len(), |e| s + 1 + e);
            raw[s + 1..end].iter().filter(|l| !l.is_empty()).count() == 1
        }
        _ => true,
    }
}

/// a "│ " gutter Claude draws down the left of a framed command (psmux, newer Claude Code)
macro_rules! GUTTER {
    () => {
        re!(r"^(\s*)[│╎┃](?: |$)")
    };
}

/// A dialog's text as its real lines. Dashed separators frame a command or file: they become one
/// '╌' marker, and a "│ " gutter inside the frame goes. Prose lines Claude wrapped at the pane width
/// are joined again; a line that ends short of the edge was a real line break and stays one.
fn tidy_body(body: &[String], width: u32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut frame: Option<Vec<String>> = None;
    // on-screen length of the previous prose line (the parser dropped its first space)
    let mut prev_len = 0usize;
    for l in body {
        if re!(r"^╌+$").is_match(l.jtrim()) {
            if let Some(f) = frame.take() {
                let gutter = f.iter().all(|x| x.is_blank() || GUTTER!().is_match(x));
                out.extend(f.into_iter().map(|x| if gutter { GUTTER!().replacen(&x, 1, "${1}").into_owned() } else { x }));
                out.push("╌".into());
            } else {
                out.push("╌".into());
                frame = Some(Vec::new());
            }
            prev_len = 0;
            continue;
        }
        if let Some(f) = frame.as_mut() {
            f.push(l.clone());
            continue;
        }
        let t = l.jtrim();
        let first = first_word(t);
        // the next word would have fit on the line above (a little slack for the dialog's padding)
        // (a long link is left for extract_links, which joins its pieces without a space)
        let wrapped = prev_len > 0
            && !t.is_empty()
            && !re!(r"^([●•◦→\-*]|\d+[.)])\s").is_match(t)
            && (prev_len + 1 + first.len16()) as i64 > width as i64 - 3
            && !re!(r"https?:\/\/\S*$").is_match(out.last().map(String::as_str).unwrap_or(""));
        if wrapped {
            let last = out.last_mut().unwrap();
            last.push(' ');
            last.push_str(t);
        } else {
            out.push(l.clone());
        }
        prev_len = if t.is_empty() { 0 } else { l.len16() + 1 };
    }
    if let Some(f) = frame {
        out.extend(f);
    }
    out
}

/// Agents other than Claude Code: working = an "esc to interrupt" line near the bottom; a prompt =
/// numbered options with an approval-style footer or question.
fn parse_generic(raw: &[String], pane_width: Option<u32>) -> ScreenState {
    // Codex marks the highlighted row with ›, Gemini CLI with ●; read them like Claude's ❯
    let lines: Vec<String> = trimmed_rows(&raw.iter().map(|l| GENERIC_CURSOR!().replacen(l.jtrim_end(), 1, "${1}❯ ${2}").into_owned()).collect::<Vec<_>>());
    let width = width_of(pane_width, &lines);
    let mut state = new_state(width);
    let tail = last_n(&lines, 14);
    if let Some(busy) = tail.iter().find(|l| INTERRUPTIBLE!().is_match(l)) {
        state.working = true;
        let a = re!(r"^[\s│|]+|[\s│|]+$", "g").replace_all(busy, "");
        let a = re!(r"^[^\w(]+\s*", "u").replacen(&a, 1, "");
        let a = re!(r"\s*[•·(]?\s*(esc|ctrl\+c) to (interrupt|cancel|stop)\)?", "i").replacen(&a, 1, "");
        state.activity = Some(a.jtrim().to_owned());
    }
    let rules: Vec<usize> = (0..lines.len()).filter(|&i| RULE!().is_match(&lines[i])).collect();
    let bottom = last_n(&lines, 24);
    let looks_like_prompt = bottom.iter().any(|l| {
        re!(r"(allow|approve|proceed|run this|apply|continue|trust)\b.*\?|press enter|enter (to )?(continue|confirm|select)|esc (to )?(cancel|deny|back)|\(y\/n\)", "i").is_match(l)
    }) || bottom.iter().any(|l| re!(r"^\s*❯ \d{1,2}\.\s").is_match(l));
    if !state.working && looks_like_prompt {
        if let Some(mut p) = parse_prompt(&lines, &rules, width).filter(|p| p.options.len() >= 2) {
            // these agents don't all take a digit as the answer; moving the highlight and pressing Enter always works
            if p.options.iter().any(|o| o.selected) {
                p.navigate = Some(true);
            }
            let after = &lines[find_last(&lines, |l| OPTION!().is_match(l)).map_or(0, |i| i + 1)..];
            if p.footer.is_none() {
                p.footer = after.iter().map(|l| l.jtrim()).find(|l| re!(r"\b(enter|esc)\b", "i").is_match(l)).map(str::to_owned);
            }
            state.prompt = Some(p);
        }
    }
    state
}

macro_rules! AGY_SPINNER {
    () => {
        re!(r"^\s*[⠀-⣿]\s+(\S.*?)\s*$")
    };
}
macro_rules! AGY_MODE {
    () => {
        re!(r"(?:^|\s{2,})(accept-edits|plan)\s+·\s")
    };
}
macro_rules! AGY_NAV {
    () => {
        re!(r"↑\/↓ Navigate")
    };
}
macro_rules! AGY_FOOTER {
    () => {
        re!(r"(?:^|\s{2,})(?:(?:accept-edits|plan) · )?([^\s·]+(?: [^\s·]+)*) · (low|medium|high|xhigh|max)(?: · [^·]+)*?\s*$")
    };
}
macro_rules! AGY_ITEM {
    () => {
        re!(r"^(\s*)(>\s+)?(?:(\d{1,2})\.\s+)?(\S.*)$")
    };
}

/// Antigravity CLI (agy): an input box (rule, "> text", rule) with the footer under it; while it
/// works, a braille spinner ("⣽  Generating...") above the box, and queued messages as "▸ …".
/// Its slash and @ pickers open under the box. Approval and trust menus replace the box: "> " marks
/// the highlighted item (numbered for approvals, plain for the trust question) with a
/// "↑/↓ Navigate" line below.
fn parse_antigravity(raw: &[String], pane_width: Option<u32>) -> ScreenState {
    let lines = trimmed_rows(raw);
    let width = width_of(pane_width, &lines);
    let mut state = new_state(width);
    let last_line = lines.last().map(String::as_str);
    // the footer's right side always names the model and effort: "[plan · ]Gemini 3.8 Flash · high"
    if let Some(foot) = last_line.and_then(|l| AGY_FOOTER!().captures(l)) {
        state.model = Some(foot[1].to_owned());
        state.effort = Some(foot[2].to_owned());
    }
    // "· 1 task(s) · /tasks" after the effort: shells agy left running, which die with it
    if let Some(tasks) = last_line.and_then(|l| re!(r" · (\d+) task\(s\) · \/tasks\s*$").captures(l)) {
        let n = crate::js::to_number(&tasks[1]);
        if n > 0.0 {
            state.background = Some(Some(format!("{} background {}", &tasks[1], if n == 1.0 { "task" } else { "tasks" })));
        }
    }
    if let Some(remote) = agy_remote(&lines, width) {
        state.remote = Some(remote);
    }
    let rules: Vec<usize> = (0..lines.len()).filter(|&i| RULE!().is_match(&lines[i])).collect();

    for k in (0..rules.len().saturating_sub(1)).rev() {
        let top = rules[k];
        let bottom = rules[k + 1];
        if !re!(r"^\s*>(\s|$)").is_match(lines.get(top + 1).map(String::as_str).unwrap_or("")) || bottom - top > 15 {
            continue;
        }
        // only footer and picker lines may follow the box; a menu below means it's a past message, framed
        let after = &lines[bottom + 1..];
        if after.len() > 16 || after.iter().any(|l| re!(r"^\s*>\s+\d{1,2}\.\s").is_match(l) || (AGY_NAV!().is_match(l) && !l.contains("tab Complete"))) {
            break;
        }
        state.input_visible = Some(true);
        let rows = &lines[top + 1..bottom];
        let draft = join_box(rows, width, re!(r"^\s*>\s?"));
        // an empty box in plan or accept-edits mode shows a grey hint, not something you typed
        let d = draft.jtrim();
        if !d.is_empty() && !re!(r"^[\w-]+ mode: .*\(shift\+tab to cycle\)$").is_match(d) {
            state.draft = Some(d.to_owned());
        }
        state.mode = Some("default".into());
        for l in after {
            if let Some(m) = AGY_MODE!().captures(l) {
                state.mode = Some(m[1].to_owned());
            }
        }
        // queued messages ("▸ text", wrapped lines indented) sit between the spinner and the box. A
        // folded thought ("▸ Thought for 4s") looks the same, so only trust them under a spinner.
        let mut queue: Vec<String> = Vec::new();
        let mut wrap = String::new();
        let mut i = top as isize - 1;
        while i >= (top as isize - 16).max(0) {
            let l = &lines[i as usize];
            i -= 1;
            if let Some(q) = re!(r"^▸\s+(.*)$").captures(l) {
                queue.insert(0, format!("{}{wrap}", q[1].jtrim()));
                wrap.clear();
                continue;
            }
            if re!(r"^ {2}\S").is_match(l) {
                wrap = format!(" {}{wrap}", l.jtrim());
                continue;
            }
            break;
        }
        for i in (0..top).rev() {
            if let Some(m) = AGY_SPINNER!().captures(&lines[i]) {
                state.working = true;
                state.activity = Some(re!(r"\.{3}$|…$").replacen(&m[1], 1, "").into_owned());
                if !queue.is_empty() {
                    state.queue = Some(queue);
                }
                break;
            }
            if RULE!().is_match(&lines[i]) || re!(r"^\s*>\s").is_match(&lines[i]) {
                break;
            }
        }
        return state;
    }

    state.prompt = parse_agy_menu(&lines);
    state
}

/// agy's list pickers (/rewind, /resume): a title at column 0, items indented two ("> " on the
/// highlighted one), a deeper line under an item for its detail ("(7m ago)"), a "[2-9 of 9 items]"
/// counter when the list scrolls, and "Keyboard: ↑/↓ Navigate  enter Select  esc Cancel" last.
fn parse_agy_picker(tail: &[String], nav_at: usize) -> Option<ScreenPrompt> {
    let mut options: Vec<PromptOption> = Vec::new();
    let mut detail: Option<String> = None;
    let mut title = String::new();
    for i in (0..nav_at).rev() {
        let l = &tail[i];
        if l.is_blank() || re!(r"^\s*\[\d+-\d+ of \d+ items?\]$").is_match(l) {
            continue;
        }
        if let Some(sub) = re!(r"^ {4,}(\S.*)$").captures(l) {
            detail = Some(match detail {
                Some(d) => format!("{} {d}", sub[1].jtrim()),
                None => sub[1].jtrim().to_owned(),
            });
            continue;
        }
        if let Some(item) = re!(r"^(> | {2})(\S.*)$").captures(l) {
            let mut o = opt(0, item[2].jtrim(), &item[1] == "> ");
            o.detail = detail.take().map(|d| re!(r"^\((.*)\)$").replacen(&d, 1, "${1}").into_owned());
            options.insert(0, o);
            continue;
        }
        title = l.jtrim().to_owned();
        break;
    }
    // /model: the search field isn't an item, the effort slider under the list isn't either (its
    // ←/→ keys work on it), and "(current)" marks the model in use
    let mut i = options.len();
    while i > 0 {
        i -= 1;
        let o = &mut options[i];
        if re!(r"^Search:?$").is_match(&o.label) || re!(r"^Effort\s+◂").is_match(&o.label) {
            options.remove(i);
        } else if re!(r"\s{2,}\(current\)$").is_match(&o.label) {
            o.label = re!(r"\s{2,}\(current\)$").replacen(&o.label, 1, "").into_owned();
            o.detail = Some(["current".to_owned()].into_iter().chain(o.detail.take().filter(|d| !d.is_empty())).collect::<Vec<_>>().join(" · "));
        }
    }
    // a table (/resume: title, folder, steps, age) has a heading and a search line above its rows
    let mut rows = options;
    if rows.iter().any(|o| re!(r" {3,}").is_match(&o.label)) {
        let head: Vec<&PromptOption> = rows.iter().filter(|o| !re!(r" {3,}").is_match(&o.label) && !re!(r"^Type to search", "i").is_match(&o.label)).collect();
        if let Some(h) = head.last() {
            title = h.label.clone();
        }
        rows.retain(|o| re!(r" {3,}").is_match(&o.label));
        for o in rows.iter_mut() {
            let parts: Vec<String> = split_plain(&o.label, re!(r" {3,}")).into_iter().map(str::to_owned).collect();
            o.label = parts[0].clone();
            let detail: Vec<String> = parts[1..].iter().cloned().chain(o.detail.take()).filter(|s| !s.is_empty()).collect();
            o.detail = Some(detail.join(" · "));
        }
    }
    for (i, o) in rows.iter_mut().enumerate() {
        o.n = i as u64 + 1;
    }
    if rows.is_empty() || !rows.iter().any(|o| o.selected) {
        return None;
    }
    let mut p = prompt(if title.is_empty() { vec![] } else { vec![title] }, rows);
    p.footer = Some(tail[nav_at].jtrim().to_owned());
    p.navigate = Some(true);
    Some(p)
}

/// The latest /remote-control answer on screen: "Remote control on for this session." with an
/// "Open <link> on another device" line (wrapped at the pane edge when long), or "Remote control off."
/// None when there's none, `Some(None)` when it's off (or on without a link).
fn agy_remote(lines: &[String], width: u32) -> Option<Option<String>> {
    for i in (0..lines.len()).rev() {
        if re!(r"^\s*(?:⎿\s+)?Remote control off\b").is_match(&lines[i]) {
            return Some(None);
        }
        if !re!(r"^\s*(?:⎿\s+)?Remote control on for this session").is_match(&lines[i]) {
            continue;
        }
        let mut text = String::new();
        for j in i + 1..lines.len().min(i + 8) {
            let l = &lines[j];
            text += if j > i + 1 && lines[j - 1].len16() as i64 >= width as i64 - 1 { "" } else { " " };
            text += l.jtrim();
            if l.contains("on another device") {
                break;
            }
        }
        return Some(re!(r"https:\/\/\S+").find(&text).map(|m| m.as_str().to_owned()));
    }
    None
}

struct Row {
    indent: usize,
    n: Option<u64>,
    label: String,
    selected: bool,
    full: bool,
}

/// `r.n` as JavaScript tests it: 0 and undefined are both false.
fn has_n(n: Option<u64>) -> bool {
    n.is_some_and(|n| n != 0)
}

fn parse_agy_menu(lines: &[String]) -> Option<ScreenPrompt> {
    let tail = last_n(lines, 30);
    let nav_at = find_last(tail, |l| AGY_NAV!().is_match(l))?;
    if tail[nav_at].contains("tab Complete") {
        return None;
    }
    if re!(r"^Keyboard:.*enter Select").is_match(tail[nav_at].jtrim()) {
        return parse_agy_picker(tail, nav_at);
    }
    let mut end = nav_at as isize - 1;
    while end >= 0 && tail[end as usize].is_blank() {
        end -= 1;
    }
    // walk up through the items (all at one indent); wrapped labels continue deeper
    let width = lines.iter().map(|l| l.len16()).max().unwrap_or(0) as i64;
    let parse = |l: &str| -> Option<Row> {
        if l.is_blank() {
            return None;
        }
        let m = AGY_ITEM!().captures(l)?;
        Some(Row {
            indent: m[1].len16() + group(&m, 2).map_or(0, |s| s.len16()),
            n: group(&m, 3).filter(|s| !s.is_empty()).map(|s| crate::js::to_number(s) as u64),
            label: m[4].jtrim().to_owned(),
            selected: m.get(2).is_some(),
            full: l.len16() as i64 >= width - 1,
        })
    };
    let mut rows: Vec<Row> = Vec::new();
    let last = if end >= 0 { parse(&tail[end as usize]) } else { parse("") };
    let item_indent: isize = last.as_ref().map_or(-1, |r| r.indent as isize);
    let last_n_val = last.as_ref().and_then(|r| r.n);
    let mut first = end + 1;
    let mut i = end;
    while i >= 0 {
        let Some(r) = parse(&tail[i as usize]) else { break };
        if has_n(last_n_val) {
            // numbered (approvals): a long label wraps onto lines of any indent, even column 0
            if has_n(r.n) && r.indent as isize != item_indent {
                break;
            }
        } else if (r.indent as isize) < item_indent || (r.indent as isize > item_indent && has_n(r.n)) {
            break;
        }
        let one = r.n == Some(1);
        rows.insert(0, r);
        first = i;
        if one {
            break;
        }
        i -= 1;
    }
    let numbered = rows.iter().any(|r| has_n(r.n));
    let mut options: Vec<PromptOption> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        // wrapped label: the terminal drops the space at the edge, so join with one unless it broke after a hyphen
        if numbered && !has_n(r.n) && !options.is_empty() {
            let prev_full = i > 0 && rows[i - 1].full;
            let o = options.last_mut().unwrap();
            let sep = if prev_full && o.label.ends_with('-') { "" } else { " " };
            o.label = format!("{}{sep}{}", o.label, r.label);
        } else if numbered && !has_n(r.n) {
            continue;
        } else {
            let n = r.n.unwrap_or(options.len() as u64 + 1);
            options.push(opt(n, r.label.clone(), r.selected));
        }
    }
    if options.len() < 2 || !options.iter().any(|o| o.selected) {
        return None;
    }
    // the text above the menu, up to a blank pair or a rule
    let mut above: Vec<String> = Vec::new();
    let mut i = first - 1;
    let mut blanks = 0;
    while i >= 0 && above.len() < 12 {
        let l = &tail[i as usize];
        if RULE!().is_match(l) {
            break;
        }
        blanks = if l.is_blank() { blanks + 1 } else { 0 };
        if blanks >= 2 {
            break;
        }
        above.insert(0, l.clone());
        i -= 1;
    }
    while above.first().is_some_and(|l| l.is_blank()) {
        above.remove(0);
    }
    let mut body: Vec<String> = above.into_iter().filter(|l| !l.is_blank()).collect();
    // "Requesting permission for: <command>  Run this command?" → question, then the command framed
    if let Some(ask) = body.iter().position(|l| re!(r"^Requesting permission for:?$", "i").is_match(l.jtrim())) {
        let question = body[ask + 1..].iter().find(|l| l.jtrim().ends_with('?')).cloned();
        let mut detail: Vec<String> = body[ask + 1..].iter().filter(|l| Some(*l) != question.as_ref()).cloned().collect();
        let indent = detail.iter().map(|l| l.indent16()).min().unwrap_or(0);
        for d in detail.iter_mut() {
            *d = d.from16(indent as isize).to_owned();
        }
        let q = question.as_deref().map(|q| q.jtrim().to_owned()).unwrap_or_else(|| body[ask].jtrim().to_owned());
        body = [q, "╌".into()].into_iter().chain(detail).chain(["╌".into()]).collect();
    }
    let mut p = prompt(body, options);
    p.footer = Some(tail[nav_at].jtrim().to_owned());
    p.navigate = Some(true);
    Some(p)
}

// ---------- Codex ----------

/// "GPT-6-Luna high · ~/project" under the composer; the effort reads "default" until you pick one
macro_rules! CX_FOOTER {
    () => {
        re!(r"^ {2}(\S.*?) (default|none|minimal|low|medium|high|xhigh|max|ultra) · [~/]")
    };
}
/// "• Working (1m 16s • esc to interrupt) · 1 background terminal running": the verb, the time so far, the rest
macro_rules! CX_WORKING {
    () => {
        re!(r"^•\s+(.+?)\s+\(((?:\d+h ?)?(?:\d+m ?)?(?:\d+s)?)[^•)]*•\s*esc to interrupt\)(.*)$")
    };
}
macro_rules! CX_NAV {
    () => {
        re!(r"^\s*(Press enter to confirm|enter (select|default|continue|confirm|submit)|.*\besc (back|to cancel)\b)")
    };
}
/// a menu row, "› 1. Yes" highlighted; a question's rows sit indented under "Queued follow-up inputs"
macro_rules! CX_OPTION {
    () => {
        re!(r"^ {0,4}(›)? ?(\d{1,2})\. (.*)$")
    };
}
/// "? 1 question" under the working line: Codex asked something and waits (shift+← opens it)
macro_rules! CX_QUESTIONS {
    () => {
        re!(r"^ {2}\? (\d+) questions?$")
    };
}

/// "1m 16s" → milliseconds
fn elapsed_ms(s: &str) -> f64 {
    let mut ms = 0.0;
    for c in re!(r"(\d+)([hms])", "g").captures_iter(s) {
        let unit = match &c[2] {
            "h" => 3_600_000.0,
            "m" => 60_000.0,
            _ => 1000.0,
        };
        ms += crate::js::to_number(&c[1]) * unit;
    }
    ms
}

/// "N background terminal(s)"
fn terminals(n: &str) -> String {
    format!("{n} background {}", if crate::js::to_number(n) == 1.0 { "terminal" } else { "terminals" })
}

/// Codex 0.16x: transcript lines start with "•" (or "›" for your messages), then the composer
/// ("› text", wrapped lines indented two), a footer with the model, effort and folder ("… Plan mode"
/// at the end in Plan mode) and a hint line. While it works, "• Working (5s • esc to interrupt)"
/// sits above the composer, then "• Queued follow-up inputs" with "↳ text" rows. Menus (approvals,
/// /model, /permissions) replace the composer: a title, "› 1. Option  detail" rows and a key hint.
fn parse_codex(raw: &[String], ghost: &[bool], pane_width: Option<u32>) -> ScreenState {
    let lines = trimmed_rows(raw);
    let width = width_of(pane_width, &lines);
    let mut state = new_state(width);
    let foot_at = (0..lines.len()).rev().find(|&i| i + 4 >= lines.len() && CX_FOOTER!().is_match(&lines[i]));
    let mut bx: Option<usize> = None;
    if let Some(foot_at) = foot_at.filter(|&f| f > 0) {
        for i in (0..foot_at).rev() {
            // "»" instead of "›" when Ultra effort lets Codex delegate
            if re!(r"^[›»!] ").is_match(&lines[i]) || re!(r"^[›»!]$").is_match(&lines[i]) {
                // a highlighted menu row is not the composer
                if !CX_OPTION!().is_match(&lines[i]) {
                    bx = Some(i);
                }
                break;
            }
            if !lines[i].is_empty() && !lines[i].starts_with("  ") {
                break;
            }
        }
    }
    let Some(bx) = bx else {
        state.prompt = parse_codex_menu(&lines);
        return state;
    };
    let foot_at = foot_at.unwrap();
    let foot = CX_FOOTER!().captures(&lines[foot_at]).unwrap();
    state.input_visible = Some(true);
    state.model = Some(foot[1].to_owned());
    state.effort = Some(foot[2].to_owned());
    if lines[foot_at].ends_with(" Plan mode") {
        state.mode = Some("plan".into());
    }
    let mut rows: Vec<String> = lines[bx..foot_at].to_vec();
    while rows.len() > 1 && rows.last().is_some_and(|l| l.is_empty()) {
        rows.pop();
    }
    let draft = join_box(&rows, width, re!(r"^[›»]\s?"));
    // the empty composer shows a dimmed suggestion ("Ask Codex to do anything")
    if !draft.is_blank() && !ghost.get(bx).copied().unwrap_or(false) {
        state.draft = Some(draft.jtrim().to_owned());
    }

    let mut queue: Vec<String> = Vec::new();
    let mut wrap = String::new();
    for i in (0..bx).rev() {
        let l = &lines[i];
        if let Some(w) = CX_WORKING!().captures(l) {
            state.working = true;
            state.activity = Some(w[1].to_owned());
            // the time so far, whole seconds: when the turn started, give or take the redraw
            if !w[2].is_empty() {
                state.since = Some((js_round((crate::clock::now_ms() as f64 - elapsed_ms(&w[2])) / 1000.0) * 1000.0) as u64);
            }
            if let Some(bg) = re!(r"(\d+) background terminals? running").captures(&w[3]) {
                state.background = Some(Some(terminals(&bg[1])));
            }
            break;
        }
        let q = re!(r"^ {2}↳ (.*)$").captures(l);
        if let Some(asked) = CX_QUESTIONS!().captures(l) {
            state.questions = Some(asked[1].parse().unwrap_or(0));
        } else if let Some(q) = q {
            queue.insert(0, format!("{}{wrap}", q[1].jtrim()));
            wrap.clear();
        } else if re!(r"^ {4}\S").is_match(l) {
            if !re!(r"edit last queued|to answer$").is_match(l) {
                wrap = format!(" {}{wrap}", l.jtrim());
            }
        } else if re!(r"^[›»•] ").is_match(l) && l != "• Queued follow-up inputs" {
            break;
        }
    }
    if state.working && !queue.is_empty() {
        state.queue = Some(queue);
    }
    if !state.working {
        state.questions = None;
        // idle, with shells still running in the background
        let tail = lines[bx.saturating_sub(6)..bx].join(" ");
        if let Some(bg) = re!(r"(\d+) background terminals? running").captures(&tail) {
            state.background = Some(Some(terminals(&bg[1])));
        }
    }
    // the composer was read: nothing listed means nothing running
    if state.background.is_none() {
        state.background = Some(None);
    }
    state
}

/// An approval or picker: title and body lines, numbered options with a detail column, a key hint.
fn parse_codex_menu(lines: &[String]) -> Option<ScreenPrompt> {
    let tail = last_n(lines, 40);
    let nav_at = find_last(tail, |l| CX_NAV!().is_match(l))?;
    let mut options: Vec<PromptOption> = Vec::new();
    let mut i = nav_at as isize - 1;
    while i >= 0 && tail[i as usize].is_blank() {
        i -= 1;
    }
    let mut wrap = String::new();
    while i >= 0 {
        let l = &tail[i as usize];
        if let Some(m) = CX_OPTION!().captures(l) {
            let parts = split_plain(&m[3], re!(r" {2,}"));
            let detail: Vec<&str> = parts[1..].iter().copied().chain([wrap.jtrim()]).filter(|s| !s.is_empty()).collect();
            let detail = detail.join(" ");
            // "(y)" is the option's key, "(current)" marks what's set now
            let current = parts[0].contains("(current)");
            let label = re!(r"\s*\((?:y|p|a|n|esc)\)\s*$", "i").replacen(parts[0], 1, "");
            let label = re!(r"\s*\(current\)").replacen(&label, 1, "");
            let mut o = opt(crate::js::to_number(&m[2]) as u64, label.jtrim(), m.get(1).is_some_and(|g| g.as_str() == "›"));
            if !detail.is_empty() {
                o.detail = Some(detail);
            }
            if current {
                o.current = Some(true);
            }
            options.insert(0, o);
            wrap.clear();
            i -= 1;
            continue;
        }
        // a wrapped label, deeper than the rows (a question's rows are themselves indented by four)
        if re!(r"^ {4,}\S").is_match(l) && !(!options.is_empty() && options.iter().any(|o| o.selected) && re!(r"^ {4}\d{1,2}\. ").is_match(l)) {
            wrap = format!(" {}{wrap}", l.jtrim());
            i -= 1;
            continue;
        }
        break;
    }
    // a submenu can have a single row ("Advanced Reasoning": Max)
    if options.is_empty() || !options.iter().any(|o| o.selected) {
        return None;
    }
    // the body: the dialog's own lines above the options. Codex leaves two blank lines between the
    // transcript and a dialog (and between a dialog's text and its options), one at most inside it;
    // a turn's "Worked for 8s • 22:42" trailer and transcript entries ("• …", "› …") end it too
    let mut body: Vec<String> = Vec::new();
    let mut blanks = 0;
    let mut j = i;
    while j >= (i - 24).max(0) {
        let l = &tail[j as usize];
        j -= 1;
        if l.is_blank() {
            if !body.is_empty() {
                blanks += 1;
                if blanks >= 2 {
                    break;
                }
            }
            continue;
        }
        blanks = 0;
        if re!(r"^[›•] ").is_match(l) || re!(r"^\s*Worked for \d").is_match(l) {
            break;
        }
        // a dialog on a fresh screen sits under Codex's banner (">_ OpenAI Codex (v…)" and the folder)
        if re!(r"^\s*>_ OpenAI Codex\b").is_match(l) {
            if !body.is_empty() && re!(r"^[~/]").is_match(&body[0]) {
                body.remove(0);
            }
            break;
        }
        body.insert(0, l.jtrim().to_owned());
    }
    let mut p = prompt(body, options);
    p.footer = Some(tail[nav_at].jtrim().to_owned());
    p.navigate = Some(true);
    Some(p)
}

/// What [`codex_browsing`] found: the chosen past message's first line and the whole screen.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct CodexBrowsing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Codex's rewind browser (Esc Esc: "Browsing transcript · ↑↓/jk scroll · ←→/hl prompts · ↵ rewind
/// · esc back"; older versions say "Browsing ·") shows the chosen past message in reverse video
/// after its "›". Returns that message's first line, or None when not browsing.
pub fn codex_browsing(captured: &[String]) -> Option<CodexBrowsing> {
    let text: Vec<String> = captured.iter().map(|l| unstyle(l).text.jtrim_end().to_owned()).collect();
    if !last_n(&text, 4).iter().any(|l| re!(r"^\s*Browsing(?: transcript)? · ").is_match(l)) {
        return None;
    }
    for i in (0..captured.len()).rev() {
        if re!(r"^(?:\x1b\[[\d;]*m)*[›»] (?:\x1b\[[\d;]*m)*\x1b\[(?:0;)?7m").is_match(&captured[i]) {
            let selected = re!(r"^[›»]\s?").replacen(&text[i], 1, "").jtrim().to_owned();
            return Some(CodexBrowsing { selected: Some(selected), screen: Some(captured.join("\n")) });
        }
    }
    Some(CodexBrowsing::default())
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
