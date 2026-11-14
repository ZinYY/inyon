//! Recover Codex's drawn tables for the normal Markdown chat renderer.
//! The heavy header rule supplies display-column boundaries, including CJK cells.
use unicode_width::UnicodeWidthChar;

fn columns(line: &str, rule: char) -> Option<Vec<(usize, usize)>> {
    let mut spans = vec![];
    let mut start = None;
    for (col, ch) in line.chars().chain([' ']).enumerate() {
        if ch == rule {
            start.get_or_insert(col);
        } else if ch == ' ' {
            if let Some(begin) = start.take() {
                if col - begin < 3 { return None; }
                spans.push((begin, col));
            }
        } else { return None; }
    }
    (spans.len() >= 2).then_some(spans)
}

fn cells(line: &str, spans: &[(usize, usize)]) -> Option<Vec<String>> {
    let mut cells = vec![String::new(); spans.len()];
    let mut col = 0;
    for ch in line.chars() {
        let width = ch.width().unwrap_or(0);
        if let Some(i) = spans.iter().position(|&(start, end)| col >= start && col < end) {
            if col + width > spans[i].1 { return None; }
            cells[i].push(ch);
        } else if !ch.is_whitespace() { return None; }
        col += width;
    }
    Some(cells.into_iter().map(|s| s.trim().to_owned()).collect())
}

fn append(row: &mut [String], next: Vec<String>) {
    for (cell, part) in row.iter_mut().zip(next) {
        if part.is_empty() { continue; }
        if !cell.is_empty() { cell.push(' '); }
        cell.push_str(&part);
    }
}

fn markdown_row(row: &[String]) -> String {
    format!("| {} |", row.iter().map(|s| s.replace('\\', "\\\\").replace('|', "\\|")).collect::<Vec<_>>().join(" | "))
}

pub(super) fn restore(text: &str) -> String {
    if !text.contains('━') { return text.to_owned(); }
    let lines: Vec<_> = text.lines().collect();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    let mut fence = None;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            if fence == Some(marker) { fence = None; }
            else if fence.is_none() { fence = Some(marker); }
        }
        let spans = if fence.is_none() { columns(lines[i], '━') } else { None };
        if let Some(spans) = spans {
            // Wrapped headers are contiguous cell rows immediately above the rule.
            let mut start = i;
            while start > 0 && !lines[start-1].trim().is_empty()
                && columns(lines[start-1], '━').is_none() && columns(lines[start-1], '─').is_none()
                && cells(lines[start-1], &spans).is_some() { start -= 1; }
            let mut header = vec![String::new(); spans.len()];
            for line in &lines[start..i] { append(&mut header, cells(line, &spans).unwrap()); }
            let mut rows = vec![];
            let mut row = vec![String::new(); spans.len()];
            let mut end = i + 1;
            while end < lines.len() && !lines[end].trim().is_empty() {
                if columns(lines[end], '─').as_ref() == Some(&spans) {
                    if row.iter().any(|s| !s.is_empty()) { rows.push(row); }
                    row = vec![String::new(); spans.len()];
                } else if let Some(parts) = cells(lines[end], &spans) {
                    append(&mut row, parts);
                } else { break; }
                end += 1;
            }
            if row.iter().any(|s| !s.is_empty()) { rows.push(row); }
            if start < i && header.iter().filter(|s| !s.is_empty()).count() >= 2 && !rows.is_empty() {
                out.truncate(out.len() - (i - start));
                if out.last().is_some_and(|s| !s.is_empty()) { out.push(String::new()); }
                out.push(markdown_row(&header));
                out.push(markdown_row(&vec!["---".to_owned(); spans.len()]));
                out.extend(rows.iter().map(|row| markdown_row(row)));
                if end < lines.len() && !lines[end].trim().is_empty() { out.push(String::new()); }
                i = end;
                continue;
            }
        }
        out.push(lines[i].to_owned());
        i += 1;
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_native_table_with_wide_text_alignment_and_wrapped_cells() {
        let input = "A relaxed afternoon:\n\nPlace         Pace       Notes\n━━━━━━━━━━━━  ━━━━━━━━━  ━━━━━━━━━━━━━━\nLa Jolla      Easy       Ocean views\n                         and coffee\n────────────  ─────────  ──────────────\n海边          Slow       Bring a layer\n\nEnjoy your visit.";
        let want = "A relaxed afternoon:\n\n| Place | Pace | Notes |\n| --- | --- | --- |\n| La Jolla | Easy | Ocean views and coffee |\n| 海边 | Slow | Bring a layer |\n\nEnjoy your visit.";
        assert_eq!(restore(input), want);
        assert_eq!(restore(want), want);
    }

    #[test]
    fn preserves_unrecognized_partial_tables_code_and_plain_prose() {
        for text in ["Place  Pace\n━━━━━  ━━━━━", "One paragraph\n━━━━━━━━━━━━━━━━", "```text\nPlace   Pace\n━━━━━━  ━━━━━\nBeach   Easy\n```", "| Place | Pace |\n| --- | --- |\n| Beach | Easy |"] {
            assert_eq!(restore(text), text);
        }
    }

    #[test]
    fn joins_wrapped_headers_and_escapes_cell_pipes() {
        let input = "Afternoon  Walking\nstop       pace\n━━━━━━━━━  ━━━━━━━━━\nBeach | A  Easy\n─────────  ─────────\nMuseum     Slow";
        assert_eq!(restore(input), "| Afternoon stop | Walking pace |\n| --- | --- |\n| Beach \\| A | Easy |\n| Museum | Slow |");
    }
}
