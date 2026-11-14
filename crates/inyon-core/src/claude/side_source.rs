//! Completed raw Markdown enriches only a uniquely matching visible reply.
use inyon_protocol::{AssistantBlock, ChatItem};
use crate::{hosts::host::shq, re};
use super::screen_reply::bare;

pub(super) fn command(pid: u32, started: f64) -> String {
    format!("for p in python3 python; do if \"$p\" -c 'import sys; sys.exit(sys.version_info[0] != 3)' </dev/null >/dev/null 2>&1; then exec \"$p\" -u -c {} {pid} {started}; fi; done",
        shq(include_str!("side_source.py")))
}

fn numbers(s: &str) -> Vec<String> {
    let mut n: Vec<_> = re!(r"[0-9]+(?:[.,][0-9]+)*").find_iter(s).map(|m| m.as_str().replace(',', "")).collect();
    n.sort();
    n
}

fn matches(native: &str, source: &str) -> bool {
    let a = bare(native);
    let b = bare(source);
    if a.is_empty() { return false; }
    let math = re!(r"\\[\[(]|\$").is_match(source);
    if math && operators(native) != operators(source) { return false; }
    if a == b { return true; }
    // Drawn fractions reorder numerator/denominator rows and remove TeX syntax.
    // Require substantial identical prose at both ends and all numeric values;
    // an ambiguous or formula-only screen stays in its native representation.
    let (a, b) = (math_content(native), math_content(source));
    math
        && a.chars().count() >= 64 && b.chars().count() >= 64
        && ((a.chars().take(24).eq(b.chars().take(24)) && a.chars().rev().take(24).eq(b.chars().rev().take(24)))
            || matching_prose(native, source))
        && numbers(native) == numbers(source)
}

fn operators(text: &str) -> Vec<char> {
    let text = text.replace("\\times", "×").replace("\\cdot", "×").replace("\\pm", "±")
        .replace("\\leq", "≤").replace("\\geq", "≥").replace('−', "-").replace('·', "×");
    let mut symbols: Vec<_> = text.lines().filter(|line| !line.chars().all(|c| matches!(c, ' ' | '|' | '-' | ':')))
        .flat_map(|line| line.trim_start().strip_prefix("- ").unwrap_or(line).chars())
        .filter(|c| matches!(c, '+' | '-' | '=' | '×' | '±' | '<' | '>' | '≤' | '≥')).collect();
    symbols.sort();
    symbols
}

fn matching_prose(native: &str, source: &str) -> bool {
    let pattern = re!(r"\\\(.*?\\\)|\\\[.*?\\\]|\$\$.*?\$\$|\$[^\n$]*?\$", "s");
    let mut chunks = vec![];
    let mut from = 0;
    for math in pattern.find_iter(source) {
        chunks.push(bare(&source[from..math.start()]));
        from = math.end();
    }
    if from == 0 { return false; }
    chunks.push(bare(&source[from..]));
    chunks.retain(|s| s.chars().count() >= 8);
    if chunks.iter().map(|s| s.chars().count()).sum::<usize>() < 64 { return false; }
    let native = bare(native);
    let mut cursor = 0;
    for chunk in chunks {
        let Some(at) = native[cursor..].find(&chunk) else { return false; };
        cursor += at + chunk.len();
    }
    true
}

fn math_content(text: &str) -> String {
    bare(&re!(r"\\(?:times|cdot|frac|dfrac|tfrac|text|mathrm|mathbf|mathit|left|right|displaystyle)\b").replace_all(text, ""))
}

fn preserve_drawn_math(text: &str) -> String {
    text.split("\n\n").map(|part| {
        let math = part.contains('\n') && (part.contains('─') || part.contains('⎡') || part.contains('⎢'))
            && !part.contains('━') && (part.contains('=') || part.contains('∫') || part.contains('∑') || part.contains('√'));
        if math && !part.contains("```") { format!("```terminal-math\n{part}\n```") } else { part.to_owned() }
    }).collect::<Vec<_>>().join("\n\n")
}

pub(super) fn enrich(items: &[ChatItem], sources: &[String]) -> Vec<ChatItem> {
    let mut out = vec![];
    let mut start = 0;
    while start < items.len() {
        let end = (start+1..items.len()).find(|&i| matches!(items[i], ChatItem::User { .. })).unwrap_or(items.len());
        let turn = &items[start..end];
        let candidates: Vec<_> = turn.iter().map(|item| {
            let ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } = item else { return None };
            let found: Vec<_> = sources.iter().enumerate().filter(|(_, s)| matches(text, s)).map(|(i, _)| i).collect();
            (found.len() == 1).then(|| found[0])
        }).collect();
        let mut used = std::collections::HashSet::new();
        for (item, candidate) in turn.iter().zip(&candidates) {
            let mut item = item.clone();
            if let ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } = &mut item {
                let matching = (*candidate).or_else(|| {
                    // A raw-formula fragment can precede its drawn replacement.
                    // Remove it only after a complete reply in this same user turn
                    // has uniquely established the source; never fill a new partial
                    // answer from an older, coincidentally matching source.
                    let prefix = math_content(text);
                    if prefix.len() < 24 { return None; }
                    let found: std::collections::HashSet<_> = candidates.iter().flatten().copied()
                        .filter(|&i| math_content(&sources[i]).starts_with(&prefix)).collect();
                    (found.len() == 1).then(|| *found.iter().next().unwrap())
                });
                if let Some(i) = matching {
                    if !used.insert(i) { continue; }
                    *text = sources[i].clone();
                } else {
                    *text = preserve_drawn_math(text);
                }
            }
            out.push(item);
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_math_matches_visible_prose_and_values_but_not_another_answer() {
        let native = "The general distance formula is d = v × t.\n\n      km   25\nd = 3 ── × ── h = 1.25 km\n      h    60\n\nYou would walk 1.25 kilometres, or 1,250 metres.";
        let source = r"The general distance formula is \(d = v \times t\).

\[d = 3\frac{\text{km}}{\text{h}} \times \frac{25}{60}\text{h} = 1.25\text{km}\]

You would walk **1.25 kilometres**, or **1,250 metres**.";
        assert!(matches(native, source));
        assert!(!matches(&native.replace("1.25", "1.5"), source));
        assert!(!matches("Unrelated message from the main conversation.", source));
        assert!(!matches(" 1\n ─\n 2", r"\[\frac{1}{2}\]"));
    }

    #[test]
    fn table_formatting_matches_without_affecting_screen_anchors() {
        assert!(matches("| Place | Pace |\n| --- | --- |\n| Beach | Easy |", "| Place | Pace |\n|---|---|\n| Beach | **Easy** |"));
    }

    #[test]
    fn unavailable_formula_source_preserves_fraction_rows_and_spaces() {
        let drawing = "      25\nd = 3 ── = 1.25\n      60";
        assert_eq!(preserve_drawn_math(drawing), format!("```terminal-math\n{drawing}\n```"));
        assert_eq!(preserve_drawn_math("A normal paragraph."), "A normal paragraph.");
    }

    #[test]
    fn formula_at_the_end_uses_all_surrounding_prose_and_preserves_operators() {
        let source = "| Duration | Distance at 3 km/h |\n|---|---|\n| 20 minutes | 1 km |\n| 30 minutes | 1.5 km |\n\nThe general formula is \\(d = v \\times t\\), with time in hours.\n\nFor 30 minutes:\n\\[\nd=3\\times\\frac{30}{60}=1.5\\text{ km}\n\\]";
        let native = "Duration Distance at 3 km/h\n20 minutes 1 km\n30 minutes 1.5 km\n\nThe general formula is d = v × t, with time in hours.\n\nFor 30 minutes:\n        30\nd = 3 × ── = 1.5 km\n        60";
        assert!(matches(native, source));
        assert!(!matches(&native.replace('×', "+"), source));
        assert!(!matches(&native.replace("1.5", "1.7"), source));
    }

    #[test]
    fn complete_math_source_removes_its_provisional_fragment_only_in_the_same_turn() {
        let user = |seq| ChatItem::User { seq, uuid: format!("u{seq}"), ts: String::new(), text: "How far can I stroll?".into(), images: 0, queued: None, shell: None };
        let assistant = |seq, text: &str| ChatItem::Assistant { seq, uuid: format!("a{seq}"), msg_id: format!("a{seq}"), ts: String::new(), model: None, block: AssistantBlock::Text { text: text.into() } };
        let source = r"Distance is \(d=v \times t\), with time in hours. At 3 km/h:

\[d=3\times\frac{20}{60}=1\]

Either makes an easy stroll before coffee.".to_owned();
        let partial = assistant(1, "Distance is d = v × t, with time in hours. At 3 km/h:\n\n\\[d=3\\times\\frac");
        let complete = assistant(2, "Distance is d = v × t, with time in hours. At 3 km/h:\n\n        20\nd = 3 × ── = 1\n        60\n\nEither makes an easy stroll before coffee.");
        let items = vec![user(0), partial.clone()];
        assert_eq!(enrich(&items, &[source.clone()]), items, "an old source cannot complete a new partial answer");
        let items = vec![user(0), partial, complete.clone(), user(3), complete];
        let result = enrich(&items, &[source.clone()]);
        assert_eq!(result.len(), 4);
        for index in [1, 3] {
            assert!(matches!(&result[index], ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } if text == &source));
        }
    }
}
