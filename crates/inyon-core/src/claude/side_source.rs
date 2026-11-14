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
    if a == b { return true; }
    // Drawn fractions reorder numerator/denominator rows and remove TeX syntax.
    // Require substantial identical prose at both ends and all numeric values;
    // an ambiguous or formula-only screen stays in its native representation.
    re!(r"\\[\[(]|\$").is_match(source)
        && a.chars().count() >= 64 && b.chars().count() >= 64
        && a.chars().take(24).eq(b.chars().take(24))
        && a.chars().rev().take(24).eq(b.chars().rev().take(24))
        && numbers(native) == numbers(source)
}

fn preserve_drawn_math(text: &str) -> String {
    text.split("\n\n").map(|part| {
        let math = part.contains('\n') && (part.contains('─') || part.contains('⎡') || part.contains('⎢'))
            && !part.contains('━') && (part.contains('=') || part.contains('∫') || part.contains('∑') || part.contains('√'));
        if math && !part.contains("```") { format!("```terminal-math\n{part}\n```") } else { part.to_owned() }
    }).collect::<Vec<_>>().join("\n\n")
}

pub(super) fn enrich(items: &[ChatItem], sources: &[String]) -> Vec<ChatItem> {
    items.iter().cloned().map(|mut item| {
        if let ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } = &mut item {
            let mut found = sources.iter().filter(|s| matches(text, s));
            if let Some(source) = found.next() {
                if found.next().is_none() { *text = source.clone(); return item; }
            }
            // When no exact source survived, keep a drawn fraction's spatial
            // meaning instead of collapsing its rows into a prose paragraph.
            *text = preserve_drawn_math(text);
        }
        item
    }).collect()
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
}
