//! The reply an agent is writing, read off its screen. Claude Code, Copilot and Antigravity write a
//! message to their records only once it's finished, but they show it on screen as it streams, so
//! the chat can show it too until the record lands.
//!
//! On all three a turn looks the same: your prompt echoed, then blocks separated by blank lines
//! (text, tool rows and their output, thinking), then a spinner or status line, then the input
//! box. The reply is the run of text blocks after the last block that isn't text.

use inyon_protocol::AgentKind;

use super::screen::{sgr_codes, unstyle};
use crate::re;

/// What the screen shows of the reply being written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenReply {
    pub text: String,
    /// the reply starts on screen (its prompt is visible), so `text` is all of it so far; otherwise
    /// its start has scrolled away and `text` carries on from what was read before
    pub whole: bool,
}

/// The agents whose replies are read this way.
pub fn reads_reply(kind: AgentKind) -> bool {
    matches!(kind, AgentKind::Claude | AgentKind::Copilot | AgentKind::Antigravity)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Text,
    Other,
}

/// The reply being written on a captured screen, or None when the screen shows none.
pub fn screen_reply(captured: &[String], kind: AgentKind, pane_width: Option<u32>) -> Option<ScreenReply> {
    if !reads_reply(kind) {
        return None;
    }
    let lines: Vec<String> = captured.iter().map(|l| dot_back(kind, l)).collect();
    let rule = re!(r"^\s*─{12,}");
    // the input box: a rule with the prompt sign on the line under it
    let input = if kind == AgentKind::Antigravity { re!(r"^>(\s|$)") } else { re!(r"^\s*❯(\s|$)") };
    let box_top = (0..lines.len().saturating_sub(1)).rev().find(|&i| rule.is_match(&lines[i]) && input.is_match(&lines[i + 1]))?;
    let prompt = match kind {
        AgentKind::Antigravity => re!(r"^> \S"),
        // Copilot puts the time at the end of your prompt; its thinking rows use the same sign
        AgentKind::Copilot => re!(r"^ ?❯ .*\s\d{1,2}:\d\d$"),
        _ => re!(r"^❯ \S"),
    };
    let echo = (0..box_top).rev().find(|&i| prompt.is_match(&lines[i]));
    let mut start = match echo {
        // past the prompt's own wrapped lines
        Some(i) => {
            let mut j = i + 1;
            while j < box_top && !lines[j].is_empty() {
                j += 1;
            }
            j
        }
        None => 0,
    };
    // the screen above the box can still hold an earlier turn when no prompt shows: start after a rule
    if echo.is_none() {
        if let Some(r) = (0..box_top).rev().find(|&i| rule.is_match(&lines[i])) {
            start = r + 1;
        }
    }
    let end = (start..box_top).find(|&i| is_status(kind, &lines[i], &captured[i])).unwrap_or(box_top);
    let first = start.min(end);
    let rows = &lines[first..end];

    // blocks, with what each is
    let mut blocks: Vec<(Block, Vec<&str>)> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_at = first;
    for (i, row) in rows.iter().map(String::as_str).chain(std::iter::once("")).enumerate() {
        if !row.is_empty() {
            if cur.is_empty() {
                cur_at = first + i;
            }
            cur.push(row);
            continue;
        }
        if cur.is_empty() {
            continue;
        }
        let alone = cur.len() == 1 && tool_name_alone(kind, cur[0], &captured[cur_at]);
        let kind_of = if alone { Block::Other } else { classify(kind, cur[0], blocks.last().map(|b| b.0)) };
        blocks.push((kind_of, std::mem::take(&mut cur)));
    }
    let from = blocks.iter().rposition(|b| b.0 == Block::Other).map_or(0, |i| i + 1);
    let width = pane_width.map(|w| w as usize).unwrap_or_else(|| lines.iter().map(|l| l.chars().count()).max().unwrap_or(80));
    let paragraphs: Vec<String> = blocks[from..].iter().map(|(_, rows)| paragraph(rows, width)).filter(|p| !p.is_empty()).collect();
    if paragraphs.is_empty() {
        return None;
    }
    // its start is on screen: right after your prompt, or after something it ran
    Some(ScreenReply { text: paragraphs.join("\n\n"), whole: echo.is_some() || from > 0 })
}

/// A captured row as text. Claude Code and Copilot blink the dot of a tool while it runs: with it
/// off the row starts with two spaces and the tool's name, drawn bold, and it would read as the
/// reply going on, command and all, until the dot came back. So the dot is put back.
fn dot_back(kind: AgentKind, raw: &str) -> String {
    let text = unstyle(raw).text.trim_end().to_owned();
    let blinked = matches!(kind, AgentKind::Claude | AgentKind::Copilot) && re!(r"^  [A-Z]").is_match(&text) && starts_bold(raw);
    if blinked {
        format!("●{}", &text[1..])
    } else {
        text
    }
}

/// A tool's row before its arguments show: Claude Code draws `● Bash` for a moment, the name bold
/// and nothing after it, then `● Bash(ls)`, the dot blinking as ever (`text` has it put back).
/// A reply's own text isn't bold, so a one-word reply still reads as the reply; the caller only
/// asks for a block of one row, as a reply can open with a bold word on its own row.
fn tool_name_alone(kind: AgentKind, text: &str, raw: &str) -> bool {
    matches!(kind, AgentKind::Claude | AgentKind::Copilot) && re!(r"^ ?● [A-Z][\w-]*\s*$").is_match(text) && starts_bold(&raw.replacen('●', " ", 1))
}

/// Whether the first character that isn't a space is drawn bold.
fn starts_bold(raw: &str) -> bool {
    first_drawn(raw).0
}

/// How the first character that isn't a space is drawn: bold, and in a colour of its own.
fn first_drawn(raw: &str) -> (bool, bool) {
    let (mut bold, mut coloured) = (false, false);
    let mut i = 0;
    while i < raw.len() {
        let rest = &raw[i..];
        if let Some(sgr) = re!(r"^\x1b\[([\d;:]*)m").captures(rest) {
            let codes = sgr_codes(&sgr[1]);
            let mut k = 0;
            while k < codes.len() {
                match codes[k] as u32 {
                    0 => (bold, coloured) = (false, false),
                    1 => bold = true,
                    22 => bold = false,
                    30..=37 | 90..=97 => coloured = true,
                    39 => coloured = false,
                    // 38;5;n and 38;2;r;g;b: what follows is the colour, not more codes
                    c @ (38 | 48) => {
                        coloured |= c == 38;
                        k += if codes.get(k + 1) == Some(&5.0) {
                            2
                        } else if codes.get(k + 1) == Some(&2.0) {
                            4
                        } else {
                            0
                        };
                    }
                    _ => {}
                }
                k += 1;
            }
            i += sgr[0].len();
            continue;
        }
        let ch = rest.chars().next().unwrap();
        if ch != ' ' {
            return (bold, coloured);
        }
        i += ch.len_utf8();
    }
    (false, false)
}

/// A row between the reply and the box that isn't the reply: a spinner, a count on the right, a
/// notice or a status line.
fn is_status(kind: AgentKind, row: &str, raw: &str) -> bool {
    match kind {
        // "✻ Julienning… (7s · ↓ 690 tokens)", "✻ Brewed for 10s · done 8:23 AM", "      45294 tokens";
        // the spinner's sign turns, through `*` too, which a Markdown bullet starts with as well: the
        // spinner draws it in its colour, a reply in the plain one
        AgentKind::Claude => {
            re!(r"^[^\s\w●⎿•\-–*#>|`]\s+[A-Z][\p{L}'’ -]*(…|\sfor\s\d)", "u").is_match(row)
                || (re!(r"^\*\s+[A-Z][\p{L}'’ -]*(…|\sfor\s\d)", "u").is_match(row) && first_drawn(raw).1)
                || re!(r"^\s{30,}\S").is_match(row)
        }
        // "⣯  Generating..."
        AgentKind::Antigravity => re!(r"^[\u{2800}-\u{28ff}]\s").is_match(row) || re!(r"^\s{30,}\S").is_match(row),
        // " /path/to/folder        Session: 0.42 AIC used"
        AgentKind::Copilot => re!(r"\sSession:\s").is_match(row) && re!(r"^ ?\S").is_match(row),
        _ => false,
    }
}

/// What a block is, from its first row (and the block before it, for one with no mark).
fn classify(kind: AgentKind, first: &str, before: Option<Block>) -> Block {
    let marked = re!(r"^ ?● (.*)$").captures(first);
    match (kind, marked) {
        // Antigravity marks only what it ran; its text has no mark
        (AgentKind::Antigravity, Some(_)) => Block::Other,
        (AgentKind::Antigravity, None) => {
            // "▸ Thought for 7s", folded
            if re!(r"^\s*[⎿└]").is_match(first) || re!(r"^\s*▸ ").is_match(first) {
                Block::Other
            } else {
                Block::Text
            }
        }
        (_, Some(m)) => {
            let rest = m.get(1).map_or("", |g| g.as_str());
            // "Bash(ls)", "Read 2 files (ctrl+o to expand)", "Tip: /skills"
            let tool = re!(r"^[A-Z][\w-]*\(").is_match(rest) || rest.contains("(ctrl+o") || re!(r"^(Tip|MCP Servers?)\b").is_match(rest);
            if tool {
                Block::Other
            } else {
                Block::Text
            }
        }
        // a thinking row, or a tool's output
        (_, None) if re!(r"^ ?(❯|∴|✻) ").is_match(first) || re!(r"^\s*[⎿└]").is_match(first) => Block::Other,
        // a paragraph of the text above, or what a tool printed
        (_, None) => before.unwrap_or(Block::Other),
    }
}

/// A block's rows as Markdown: the mark and the shared indent taken off, rows the agent wrapped at
/// the screen's edge joined again, and • bullets as Markdown ones.
fn paragraph(rows: &[&str], width: usize) -> String {
    let rows: Vec<String> = rows.iter().map(|r| re!(r"^ ?● ").replace(r, |c: &regex::Captures| " ".repeat(c[0].chars().count())).into_owned()).collect();
    let indent = rows.iter().filter(|r| !r.trim().is_empty()).map(|r| r.chars().take_while(|c| *c == ' ').count()).min().unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    let mut full = false;
    for r in &rows {
        let line: String = r.chars().skip(indent).collect();
        let bullet = re!(r"^\s*([•\-*]|\d+[.)])(\s|$)").is_match(&line);
        let line = re!(r"^(\s*)•(\s|$)").replace(&line, "$1- ").into_owned();
        match out.last_mut() {
            // the row before reached the edge: this one carries on the same line
            Some(prev) if full && !bullet => {
                prev.push(' ');
                prev.push_str(line.trim_start());
            }
            _ => out.push(line),
        }
        // within a few columns of the edge, the agent wrapped it
        full = r.chars().count() + 12 >= width;
    }
    out.join("\n").trim().to_owned()
}

/// Lowercase letters and digits only, so text read off the screen (Markdown drawn, lines wrapped)
/// compares with the record it came from.
pub fn bare(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The part of `live` that no record covers yet: leading paragraphs a recorded reply already
/// holds are dropped, since the screen keeps showing them after their record lands.
pub fn unrecorded(live: &str, recorded: &[&str]) -> String {
    let recorded: Vec<String> = recorded.iter().map(|r| bare(r)).collect();
    let paragraphs: Vec<&str> = live.split("\n\n").collect();
    let covered = paragraphs
        .iter()
        .take_while(|p| {
            let b = bare(p);
            let head: String = b.chars().take(48).collect();
            !head.is_empty() && recorded.iter().any(|r| r.contains(&head))
        })
        .count();
    paragraphs[covered..].join("\n\n")
}

/// `before` and then `now`, read a moment apart from a screen whose top scrolled away in between:
/// where `now` starts inside `before`, what came before that is kept.
pub fn carry_on(before: &str, now: &str) -> String {
    // the top of the screen can start with something else (the end of your prompt): from the
    // first paragraph `before` holds
    let paragraphs: Vec<&str> = now.split("\n\n").collect();
    for k in 0..paragraphs.len() {
        let rest = paragraphs[k..].join("\n\n");
        let head: String = rest.chars().take(60).collect();
        if head.chars().count() < 20 {
            break;
        }
        // `before` holds the head, or ends partway into it (the last read caught it being written)
        let start: String = head.chars().take(20).collect();
        let at = before.match_indices(&start).map(|(at, _)| at).find(|&at| before[at..].starts_with(&head) || rest.starts_with(&before[at..]));
        if let Some(at) = at {
            return format!("{}{rest}", &before[..at]);
        }
    }
    now.to_owned()
}

#[cfg(test)]
#[path = "screen_reply_tests.rs"]
mod tests;
