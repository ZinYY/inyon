//! Strings measured and cut the JavaScript way: `length`, `slice`, `indexOf` and `s[i]` count
//! UTF-16 code units, and `trim` removes JavaScript's white space. Pane widths are compared with
//! these lengths, so a wide or astral character counts as the TS counted it.

/// What `\s` matches in a JavaScript regex and what `trim()` removes.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub trait JsStr {
    /// `s.trim()`
    fn jtrim(&self) -> &str;
    /// `s.trimStart()`
    fn jtrim_start(&self) -> &str;
    /// `s.trimEnd()`
    fn jtrim_end(&self) -> &str;
    /// `s.length`
    fn len16(&self) -> usize;
    /// `s.slice(start, end)`, negative indexes counting from the end. A surrogate pair cut in
    /// half is left out whole (JavaScript would keep half of it, which no Rust string can hold).
    fn slice16(&self, start: isize, end: Option<isize>) -> &str;
    /// `s.slice(start)`
    fn from16(&self, start: isize) -> &str {
        self.slice16(start, None)
    }
    /// `s.slice(0, end)`
    fn to16(&self, end: isize) -> &str {
        self.slice16(0, Some(end))
    }
    /// `s.indexOf(needle, from)` in UTF-16 units
    fn index_of16(&self, needle: &str, from: usize) -> Option<usize>;
    /// `s[i]` (a whole character where JS would give half a surrogate pair)
    fn at16(&self, i: usize) -> Option<char>;
    /// the byte offset of UTF-16 index `i` (clamped to the end); inside a surrogate pair, the
    /// offset after it
    fn byte_at16(&self, i: usize) -> usize;
    /// the byte offset of UTF-16 index `i` (clamped to the end); inside a surrogate pair, the
    /// offset before it
    fn byte_floor16(&self, i: usize) -> usize;
    /// the UTF-16 index of byte offset `b`
    fn idx16(&self, b: usize) -> usize;
    /// `l.length - l.trimStart().length`
    fn indent16(&self) -> usize {
        self.len16() - self.jtrim_start().len16()
    }
    /// `s.split(/\s/)[0]` and friends need the first run of non-space
    fn is_blank(&self) -> bool {
        self.jtrim().is_empty()
    }
}

impl JsStr for str {
    fn jtrim(&self) -> &str {
        self.trim_matches(is_space)
    }
    fn jtrim_start(&self) -> &str {
        self.trim_start_matches(is_space)
    }
    fn jtrim_end(&self) -> &str {
        self.trim_end_matches(is_space)
    }
    fn len16(&self) -> usize {
        if self.is_ascii() {
            return self.len();
        }
        self.chars().map(char::len_utf16).sum()
    }
    fn slice16(&self, start: isize, end: Option<isize>) -> &str {
        let len = self.len16() as isize;
        let norm = |i: isize| if i < 0 { (len + i).max(0) } else { i.min(len) } as usize;
        let from = norm(start);
        let to = end.map_or(len as usize, norm);
        if from >= to {
            return "";
        }
        // half a pair at the start is skipped, half a pair at the end left off
        let a = self.byte_at16(from);
        let b = self.byte_floor16(to);
        &self[a..b.max(a)]
    }
    fn index_of16(&self, needle: &str, from: usize) -> Option<usize> {
        let start = self.byte_at16(from);
        self[start..].find(needle).map(|b| self.idx16(start + b))
    }
    fn at16(&self, i: usize) -> Option<char> {
        let b = self.byte_at16(i);
        if self.idx16(b) != i {
            return None;
        }
        self[b..].chars().next()
    }
    fn byte_at16(&self, i: usize) -> usize {
        if self.is_ascii() {
            return i.min(self.len());
        }
        let mut units = 0;
        for (at, c) in self.char_indices() {
            if units >= i {
                return at;
            }
            units += c.len_utf16();
            // inside a pair: after it (`byte_floor16` gives the offset before it)
            if units > i {
                return at + c.len_utf8();
            }
        }
        self.len()
    }
    fn byte_floor16(&self, i: usize) -> usize {
        let b = self.byte_at16(i);
        if self.idx16(b) == i.min(self.len16()) {
            return b;
        }
        self[..b].char_indices().next_back().map_or(0, |(at, _)| at)
    }
    fn idx16(&self, b: usize) -> usize {
        self[..b.min(self.len())].len16()
    }
}

/// `s.split(/\s/)[0]`: text up to the first JavaScript space.
pub fn first_word(s: &str) -> &str {
    s.split(is_space).next().unwrap_or("")
}

/// `s.split(/\s/).pop()`: text after the last JavaScript space.
pub fn last_word(s: &str) -> &str {
    s.rsplit(is_space).next().unwrap_or("")
}

/// `s.replace(/\s+/g, '')`
pub fn squash(s: &str) -> String {
    s.chars().filter(|c| !is_space(*c)).collect()
}

/// `s.replace(/\s+$/, '')`
pub fn trim_end(s: &str) -> String {
    s.jtrim_end().to_owned()
}

/// `s.toLowerCase()` (Unicode lower case, as JavaScript's default)
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `s.charAt(0).toUpperCase() + s.slice(1)`
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `a < b` for two JavaScript strings: by UTF-16 code units.
pub fn utf16_lt(a: &str, b: &str) -> bool {
    a.encode_utf16().lt(b.encode_utf16())
}

/// `a.localeCompare(b)` for the ids it orders (ASCII letters, digits and punctuation), as ICU's
/// root collation orders them: punctuation, then digits, then letters alphabetically with case
/// ignored, and lower case first when that's the only difference.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    fn primary(c: char) -> (u8, char) {
        if c.is_ascii_digit() {
            (1, c)
        } else if c.is_alphabetic() {
            (2, c.to_lowercase().next().unwrap_or(c))
        } else {
            (0, c)
        }
    }
    let pa: Vec<_> = a.chars().map(primary).collect();
    let pb: Vec<_> = b.chars().map(primary).collect();
    pa.cmp(&pb).then_with(|| {
        // tertiary: lower case before upper case
        let ta: Vec<bool> = a.chars().map(|c| c.is_uppercase()).collect();
        let tb: Vec<bool> = b.chars().map(|c| c.is_uppercase()).collect();
        ta.cmp(&tb)
    })
}

/// `encodeURIComponent(s)`: everything but letters, digits and `-_.!~*'()` as %XX of its UTF-8.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_utf16() {
        assert_eq!("a😀b".len16(), 4);
        assert_eq!("a😀b".slice16(1, Some(3)), "😀");
        assert_eq!("abc".slice16(-2, None), "bc");
        assert_eq!("❯ x".index_of16("x", 0), Some(2));
        assert_eq!("\u{feff} a \u{85}".jtrim(), "a \u{85}");
        assert_eq!(locale_compare("prt_a", "prt_B"), std::cmp::Ordering::Less);
    }

    #[test]
    fn a_pair_cut_in_half_is_left_out_never_over_the_limit() {
        // JavaScript keeps the high half ("a\uD83D"); no Rust string can, so the pair goes
        assert_eq!("a😀b".to16(2), "a");
        assert_eq!("a😀b".slice16(2, None), "b");
        assert_eq!("a😀b".slice16(2, Some(3)), "");
        assert_eq!("😀😀".to16(3), "😀");
        assert_eq!(("a😀b".byte_at16(2), "a😀b".byte_floor16(2), "a😀b".byte_floor16(3), "a😀b".byte_floor16(9)), (5, 1, 5, 6));
        for n in 0..12 {
            assert!("x😀y😀😀z".to16(n).len16() <= n as usize);
        }
    }
}
