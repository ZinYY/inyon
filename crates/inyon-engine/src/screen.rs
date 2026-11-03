//! The one thing the link reads off a Claude Code screen: what its footer lists as still running
//! in the background ("1 shell, 1 agent"), so a finished notice waits for it. The part of the
//! hub's claude/screen.ts that `backgroundOnScreen` uses, with the same patterns.

use std::sync::OnceLock;

use regex::Regex;

struct Patterns {
    /// a session named with /rename shows its name in the rule above the input box: "──── Name ─"
    rule: Regex,
    option: Regex,
    input: Regex,
    /// "1 shell", "2 monitors", "← 1 agent", "1 local agent", "1 background dynamic workflow"
    background: Regex,
    /// the agent list under the footer: "● main", "◯ general-purpose  Sleep briefly  6s"
    agent_row: Regex,
    sgr: Regex,
    other_escape: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        rule: Regex::new(r"^\s*─{12,}(?: [^─]{1,100} ─{1,12})?\s*$").unwrap(),
        option: Regex::new(r"^(\s*)(❯\s*)?(\d{1,2})\.\s(.*)$").unwrap(),
        input: Regex::new(r"^\s*[❯>!](\s|$)").unwrap(),
        background: Regex::new(
            r"\b\d+ (?:local |background |remote |MCP |Artifact comment )?(?:shells?|monitors?|agents?|tasks?|terminals?|teams?|cloud sessions?|(?:dynamic )?workflows?|bash)\b",
        )
        .unwrap(),
        agent_row: Regex::new(r"^\s*[●◯]\s").unwrap(),
        sgr: Regex::new(r"^\x1b\[[\d;:]*m").unwrap(),
        other_escape: Regex::new(r"^\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[\d;?]*[A-Za-z]|.)").unwrap(),
    })
}

/// A captured line without its terminal styling (tmux capture-pane -e).
pub fn unstyle(line: &str) -> String {
    if !line.contains('\x1b') {
        return line.to_string();
    }
    let p = patterns();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with('\x1b') {
            let skip = p.sgr.find(rest).or_else(|| p.other_escape.find(rest)).map(|m| m.end()).unwrap_or(1);
            i += skip.max(1);
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// What a Claude pane's footer lists as running in the background: `Some(None)` for nothing,
/// `None` when the footer isn't visible.
pub fn background_on_screen(captured: &[&str]) -> Option<Option<String>> {
    let p = patterns();
    let mut lines: Vec<String> = captured.iter().map(|l| unstyle(l).trim_end().to_string()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let rules: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| p.rule.is_match(l)).map(|(i, _)| i).collect();
    // the idle or working layout: rule, "❯ input", rule, status lines
    for k in (0..rules.len().saturating_sub(1)).rev() {
        let top = rules[k];
        let next = lines.get(top + 1).map(String::as_str).unwrap_or("");
        if p.input.is_match(next) && !p.option.is_match(next) && rules[k + 1] - top <= 15 {
            // an agent's description in the list under the footer could say anything
            let found: Vec<&str> = lines[rules[k + 1] + 1..].iter().filter(|l| !p.agent_row.is_match(l)).flat_map(|l| p.background.find_iter(l).map(|m| m.as_str())).collect();
            // the footer was read: nothing listed means nothing running
            return Some((!found.is_empty()).then(|| found.join(", ")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> String {
        "─".repeat(100)
    }

    fn footer(status: &str, rows: &[&str]) -> String {
        let r = rule();
        let mut s = format!("● Started the agent in the background.\n{r}\n❯\n{r}\n  {status}");
        for row in rows {
            s.push_str(&format!("\n  {row}"));
        }
        s
    }

    fn bg(text: &str) -> Option<Option<String>> {
        background_on_screen(&text.split('\n').collect::<Vec<_>>())
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/../../fixtures/screens/claude/{name}.txt", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    const AGENTS: [&str; 2] = ["● main", "◯ general-purpose  Sleep briefly for a footer test                                                      6s · ↓ 37.9k tokens"];

    #[test]
    fn a_shell_and_an_agent_with_bypass_mode() {
        assert_eq!(bg(&footer("⏵⏵ bypass permissions on · 1 shell · ← 1 agent", &AGENTS)), Some(Some("1 shell, 1 agent".into())));
    }

    #[test]
    fn agents_alone_and_the_list_under_them_is_not_a_footer() {
        assert_eq!(bg(&footer("← 1 agent", &[])), Some(Some("1 agent".into())));
        assert_eq!(bg(&footer("1 shell · 1 agent", &[])), Some(Some("1 shell, 1 agent".into())));
        let mut rows = AGENTS.to_vec();
        rows.push("◯ Explore  Find the parser  12s · ↓ 4.1k tokens");
        assert_eq!(bg(&footer("← 2 agents", &rows)), Some(Some("2 agents".into())));
        assert_eq!(
            bg(&footer("⏵ plan mode on · ← 1 agent", &["● main", "◯ general-purpose  Start 2 shells, then ⏵⏵ bypass permissions on  6s · ↓ 1.2k tokens"])),
            Some(Some("1 agent".into()))
        );
    }

    #[test]
    fn a_bare_footer_is_nothing_running_and_no_footer_is_unknown() {
        assert_eq!(bg(&footer("⏵⏵ bypass permissions on", &[])), Some(None));
        assert_eq!(bg(&fixture("effort-dialog")), None);
    }

    #[test]
    fn every_kind_of_background_work_claude_code_names() {
        assert_eq!(bg(&footer("⏵⏵ bypass permissions on · 1 local agent", &[])), Some(Some("1 local agent".into())));
        assert_eq!(bg(&footer("⏵⏵ auto mode on (shift+tab to cycle) · 1 background dynamic workflow", &[])), Some(Some("1 background dynamic workflow".into())));
        assert_eq!(bg(&footer("1 MCP task · 1 Artifact comment monitor · 1 cloud session", &[])), Some(Some("1 MCP task, 1 Artifact comment monitor, 1 cloud session".into())));
        assert_eq!(bg(&footer("⏵⏵ bypass permissions on · 1 shell · /tasks to see subagents · ← 1 agent", &[])), Some(Some("1 shell, 1 agent".into())));
        assert_eq!(bg(&footer("3 background tasks", &[])), Some(Some("3 background tasks".into())));
    }

    #[test]
    fn a_finished_command_leaves_its_shell_listed() {
        let text = fixture("working-tool-hints");
        let mut lines: Vec<&str> = text.split('\n').collect();
        lines[0] = "● Command finished";
        assert_eq!(background_on_screen(&lines), Some(Some("1 shell".into())));
    }

    #[test]
    fn styling_is_stripped() {
        assert_eq!(unstyle("\x1b[2m❯ \x1b[7mT\x1b[0mry\x1b]8;;http://x\x07link\x1b]8;;\x07"), "❯ Trylink");
    }
}
