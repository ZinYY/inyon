//! The few JavaScript string rules the TS code leans on, so both sides read text the same way.

/// What `\s` matches in a JavaScript regex and what `trim()` removes.
pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// A string's `length` in JavaScript: UTF-16 code units.
pub(crate) fn len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, n)` in JavaScript. A character that would be cut in half is left out whole (JS
/// would keep half of it, which no valid string can hold).
pub(crate) fn slice(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (at, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..at];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn slices_count_utf16_units() {
        assert_eq!(super::len("a\u{1f642}b"), 4);
        assert_eq!(super::slice("a\u{1f642}b", 3), "a\u{1f642}");
        assert_eq!(super::slice("a\u{1f642}b", 2), "a");
        assert_eq!(super::slice("été", 64), "été");
    }

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(super::trim("\u{feff}\u{a0} x \u{3000}\n"), "x");
        // NEL isn't JavaScript whitespace
        assert_eq!(super::trim("\u{85}x"), "\u{85}x");
    }
}
