//! An agent's question or approval as the screen reads it (`ScreenPrompt`), made into what the
//! card shows: the title, the prose and the framed command or diff, and which choice the keys
//! pick. The text rules are PromptCard.tsx's.

use inyon_protocol::{prompt_key, ScreenPrompt};

use crate::api::Answer;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(Vec<String>),
    /// a command or a file's text, framed by dashed lines on screen
    Code(Vec<String>),
}

/// The prompt's body: its first line as the title, then prose and framed blocks. Hints that only
/// make sense inside a terminal are left out.
pub fn parse_body(body: &[String]) -> (String, Vec<Block>) {
    let terminal_only = |l: &str| l.trim_start().starts_with("Tip:") || ["Hold Shift", "native copy", "iTerm2", "Terminal.app"].iter().any(|h| l.contains(h));
    let lines: Vec<String> = body.iter().filter(|l| !terminal_only(l)).map(|l| l.replace(" (c to copy)", "")).collect();
    let title = lines.first().map(|l| l.trim().to_string()).unwrap_or_default();
    let mut blocks: Vec<Block> = vec![];
    let mut in_code = false;
    for l in lines.iter().skip(1) {
        if l == "╌" {
            in_code = !in_code;
            continue;
        }
        match (blocks.last_mut(), in_code) {
            (Some(Block::Code(v)), true) | (Some(Block::Text(v)), false) => v.push(l.clone()),
            _ => blocks.push(if in_code { Block::Code(vec![l.clone()]) } else { Block::Text(vec![l.clone()]) }),
        }
    }
    blocks.retain(|b| match b {
        Block::Text(v) | Block::Code(v) => v.iter().any(|l| !l.trim().is_empty()),
    });
    (title, blocks)
}

/// Joins lines the terminal wrapped, keeping real line starts (bullets, arrows, numbered items).
pub fn join_wrapped(lines: &[String]) -> String {
    let mut out = String::new();
    for raw in lines {
        let l = raw.trim();
        if l.is_empty() {
            continue;
        }
        let starts_item = {
            let mut c = l.chars();
            let first = c.next().unwrap_or(' ');
            let bullet = "●•◦→-*".contains(first) && c.next() == Some(' ');
            let digits: String = l.chars().take_while(char::is_ascii_digit).collect();
            let numbered = !digits.is_empty() && matches!(l[digits.len()..].chars().next(), Some('.') | Some(')')) && l[digits.len() + 1..].starts_with(' ');
            bullet || numbered
        };
        let new_line = out.is_empty() || starts_item || out.ends_with(['?', ':', '.', '!']);
        if new_line {
            if !out.is_empty() {
                out.push('\n');
            }
        } else {
            out.push(' ');
        }
        out.push_str(l);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLine {
    Add,
    Del,
    Same,
}

/// Claude's edit prompt frames a diff: "12  text" (unchanged), "14 -  text" (removed), "14 +  text"
/// (added), with wrapped tails on lines of their own. Each line's kind, or None when the block
/// isn't a diff.
pub fn diff_kinds(lines: &[String]) -> Option<Vec<DiffLine>> {
    let numbered = |l: &str| -> Option<Option<char>> {
        let t = l.trim_start();
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let rest = &t[digits..];
        if rest.is_empty() {
            return Some(None);
        }
        let mut c = rest.chars();
        (c.next() == Some(' ')).then(|| c.next().filter(|s| "-+ ".contains(*s)))?.map(Some)
    };
    if !lines.iter().any(|l| matches!(numbered(l), Some(Some('+' | '-')))) {
        return None;
    }
    let mut last = DiffLine::Same;
    Some(
        lines
            .iter()
            .map(|l| {
                if let Some(sign) = numbered(l) {
                    last = match sign {
                        Some('+') => DiffLine::Add,
                        Some('-') => DiffLine::Del,
                        _ => DiffLine::Same,
                    };
                }
                last
            })
            .collect(),
    )
}

/// A framed block's lines without the terminal's one-space margin or blank edges.
pub fn code_lines(lines: &[String]) -> Vec<String> {
    let mut text: Vec<String> = lines.iter().map(|l| l.strip_prefix(' ').unwrap_or(l).to_string()).collect();
    while text.first().is_some_and(|l| l.trim().is_empty()) {
        text.remove(0);
    }
    while text.last().is_some_and(|l| l.trim().is_empty()) {
        text.pop();
    }
    text
}

/// It reads as a question (a tab strip, or a title ending in "?"), so its title is set larger.
pub fn is_question(p: &ScreenPrompt, title: &str) -> bool {
    p.tabs.as_ref().is_some_and(|t| !t.is_empty()) || title.ends_with('?')
}

/// The choice to put focus on: the one the agent has highlighted, else the first.
pub fn likely(p: &ScreenPrompt) -> usize {
    p.options.iter().position(|o| o.selected).unwrap_or(0)
}

/// What a key does on the card, with `focused` the option that has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// answer with this option number
    Pick(u64),
    /// move focus to this option
    Focus(usize),
    Cancel,
    None,
}

pub fn key_action(p: &ScreenPrompt, key: &str, focused: usize) -> KeyAction {
    let n = p.options.len();
    match key {
        "Escape" => KeyAction::Cancel,
        _ if n == 0 => KeyAction::None,
        "ArrowDown" => KeyAction::Focus((focused + 1) % n),
        "ArrowUp" => KeyAction::Focus((focused + n - 1) % n),
        "Home" => KeyAction::Focus(0),
        "End" => KeyAction::Focus(n - 1),
        "Enter" | " " => p.options.get(focused).map_or(KeyAction::None, |o| KeyAction::Pick(o.n)),
        k => match k.parse::<u64>() {
            // number keys pick by the number on the option (navigate-only menus have none)
            Ok(d) if p.navigate != Some(true) && p.options.iter().any(|o| o.n == d) => KeyAction::Pick(d),
            _ => KeyAction::None,
        },
    }
}

/// The answer for option `n` (with the typed text for a free-text option), carrying the key of the
/// question it was given for.
pub fn answer<'a>(key: &'a str, n: u64, text: Option<&'a str>) -> Answer<'a> {
    Answer { option: n as u32, text, question: Some(key) }
}

/// The key the hub checks an answer against.
pub fn key_of(p: &ScreenPrompt) -> String {
    prompt_key(&p.body, &p.options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt(v: serde_json::Value) -> ScreenPrompt {
        serde_json::from_value(v).unwrap()
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_body_splits_into_title_prose_and_the_framed_command() {
        let (title, blocks) = parse_body(&lines(&["Bash command", "╌", "  bun test src/auth", "╌", "Run the auth tests", "Tip: press c to copy (c to copy)"]));
        assert_eq!(title, "Bash command");
        assert_eq!(blocks, vec![Block::Code(lines(&["  bun test src/auth"])), Block::Text(lines(&["Run the auth tests"]))]);
        // the command keeps its indent once the terminal's margin is off (Ghostex #252)
        assert_eq!(code_lines(&lines(&["", "  if x:", "      y()", " "])), lines(&[" if x:", "     y()"]));
    }

    #[test]
    fn wrapped_lines_join_but_items_keep_their_own() {
        assert_eq!(
            join_wrapped(&lines(&["Do you want to make this", "edit to app.ts?", "1. first", "- second", "plain"])),
            "Do you want to make this edit to app.ts?\n1. first\n- second plain"
        );
    }

    #[test]
    fn an_edit_frames_a_diff() {
        let d = diff_kinds(&lines(&["12  keep", "13 -  old", "      tail of old", "13 +  new", "14  same"])).unwrap();
        assert_eq!(d, [DiffLine::Same, DiffLine::Del, DiffLine::Del, DiffLine::Add, DiffLine::Same]);
        assert_eq!(diff_kinds(&lines(&["bun test", "-v"])), None);
    }

    #[test]
    fn keys_pick_move_and_cancel() {
        let p = prompt(
            json!({ "body": ["Run bun test?"], "options": [{ "n": 1, "label": "Yes", "selected": false }, { "n": 2, "label": "Yes, and don't ask again", "selected": true }, { "n": 3, "label": "No", "selected": false }], "multi": false }),
        );
        assert_eq!(likely(&p), 1);
        assert_eq!(key_action(&p, "3", 1), KeyAction::Pick(3));
        assert_eq!(key_action(&p, "7", 1), KeyAction::None);
        assert_eq!(key_action(&p, "ArrowDown", 2), KeyAction::Focus(0));
        assert_eq!(key_action(&p, "ArrowUp", 0), KeyAction::Focus(2));
        assert_eq!(key_action(&p, "Enter", 1), KeyAction::Pick(2));
        assert_eq!(key_action(&p, "Escape", 0), KeyAction::Cancel);
        assert!(is_question(&p, "Run bun test?"));
    }

    // Ghostex #251: answers from the chat never reached the agent. The app sends the option and the
    // key of the question on screen, which the hub checks before it presses anything.
    #[test]
    fn an_answer_names_the_question_it_answers() {
        let p = prompt(
            json!({ "body": ["Which database?", "Pick one"], "options": [{ "n": 1, "label": "Postgres", "selected": true }, { "n": 2, "label": "SQLite", "selected": false }, { "n": 3, "label": "Type something", "selected": false }],
            "tabs": [{ "label": "Database", "done": false, "current": true }, { "label": "Cache", "done": false, "current": false }], "multi": false, "freeTextOption": 3 }),
        );
        let key = key_of(&p);
        assert_eq!(key, "Whichdatabase?\nPickone\n1.Postgres\n2.SQLite\n3.Typesomething".replace('\n', ""));
        let body = serde_json::to_string(&answer(&key, 2, None)).unwrap();
        assert_eq!(body, format!(r#"{{"option":2,"question":"{key}"}}"#));
        let typed = serde_json::to_string(&answer(&key, 3, Some("  Redis, with\n    a TTL"))).unwrap();
        assert_eq!(typed, format!(r#"{{"option":3,"text":"  Redis, with\n    a TTL","question":"{key}"}}"#));
    }
}
