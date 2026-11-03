//! JavaScript regular expressions on Rust's `regex` crate. The TS sources are copied as they are
//! and translated once: `\s` is JavaScript's white space (which includes U+FEFF and leaves out
//! U+0085), `\d` and `\w` are ASCII, `\b` is an ASCII word boundary, `.` stops at all four line
//! terminators, and a `[` inside a class is a plain character. Lookarounds and back references
//! have no translation; the port rewrites the few places that use them by hand.

use std::cell::RefCell;
use std::collections::HashMap;

use regex::{Captures, Regex};

/// The characters `\s` matches in JavaScript, as the inside of a character class.
const WS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";
const WORD: &str = "A-Za-z0-9_";

/// A regex literal from the TS source, compiled once per call site: `re!(r"^\s*❯")`, or with
/// flags, `re!(r"esc to", "i")`.
#[macro_export]
macro_rules! re {
    ($pat:expr) => {
        $crate::re!($pat, "")
    };
    ($pat:expr, $flags:expr) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| $crate::js::re::compile($pat, $flags))
    }};
}

/// Compiles a JavaScript pattern with its flags (`i`, `m`, `s`; `g`, `u` and `y` change nothing
/// here). Panics on a pattern with no translation, which only a bug in the port can produce.
pub fn compile(pattern: &str, flags: &str) -> Regex {
    let src = translate(pattern, flags);
    Regex::new(&src).unwrap_or_else(|e| panic!("regex /{pattern}/{flags} → {src}: {e}"))
}

thread_local! {
    static BUILT: RefCell<HashMap<(String, String), Regex>> = RefCell::new(HashMap::new());
}

/// `new RegExp(source, flags)` for a pattern built at run time, compiled once per thread.
pub fn dynamic(pattern: &str, flags: &str) -> Regex {
    BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if b.len() > 512 {
            b.clear();
        }
        b.entry((pattern.to_owned(), flags.to_owned())).or_insert_with(|| compile(pattern, flags)).clone()
    })
}

/// Escapes text for use inside a pattern, as the TS does when it builds one from a name.
pub fn escape(s: &str) -> String {
    regex::escape(s)
}

/// The Rust syntax for a JavaScript pattern.
///
/// The `m` flag becomes Rust's CRLF mode, so `^` and `$` see "\r", "\n" and "\r\n" as line ends as
/// JavaScript does. Two gaps are left: JavaScript also ends lines at U+2028 and U+2029, and it
/// finds an empty line between the "\r" and the "\n" of a CRLF, where Rust finds none. Neither
/// shows in what the readers match.
///
/// The `i` flag without `u` matches as JavaScript's does: two characters are the same when they
/// upper-case to the same single character, except that a non-ASCII one never matches an ASCII
/// one. So "ſ" (long s) is not "s" and the Kelvin sign is not "k", which Rust's `(?i)` (Unicode
/// case folding) would allow. Each letter becomes the class of the characters it matches. With
/// `u` as well, JavaScript folds case as Rust does, and `(?i)` stays.
pub fn translate(src: &str, flags: &str) -> String {
    let dot_all = flags.contains('s');
    let fold = flags.contains('i') && !flags.contains('u');
    let mut out = String::with_capacity(src.len() + 16);
    if flags.contains('i') && !fold {
        out.push_str("(?i)");
    }
    if flags.contains('m') {
        out.push_str("(?Rm)");
    }
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut class = false;
    // the position right after "[" or "[^", where JavaScript's "]" closes an empty class
    let mut class_open_at = usize::MAX;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' && i + 1 < c.len() {
            let n = c[i + 1];
            i += 2;
            match n {
                's' => push_class(&mut out, class, WS, false),
                'S' => push_class(&mut out, class, WS, true),
                'd' => push_class(&mut out, class, "0-9", false),
                'D' => push_class(&mut out, class, "0-9", true),
                'w' => push_class(&mut out, class, WORD, false),
                'W' => push_class(&mut out, class, WORD, true),
                'b' if class => out.push_str(r"\x08"),
                'b' => out.push_str(r"(?-u:\b)"),
                'B' => out.push_str(r"(?-u:\B)"),
                'n' | 't' | 'r' | 'f' | 'v' => {
                    out.push('\\');
                    out.push(n);
                }
                '0' => out.push_str(r"\x00"),
                '1'..='9' => panic!("back reference in /{src}/"),
                'x' | 'u' => {
                    let hex: String = if n == 'u' && c.get(i) == Some(&'{') {
                        let end = c[i..].iter().position(|&x| x == '}').map(|p| i + p).expect("\\u{…}");
                        let hex = c[i + 1..end].iter().collect();
                        i = end + 1;
                        hex
                    } else {
                        let len = if n == 'x' { 2 } else { 4 };
                        let hex = c[i..(i + len).min(c.len())].iter().collect();
                        i += len;
                        hex
                    };
                    out.push_str(&format!(r"\x{{{hex}}}"));
                    // a letter written as a code: its other cases, unless it starts a range
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).filter(|_| fold && !(class && c.get(i) == Some(&'-'))) {
                        push_others(&mut out, ch, class);
                    }
                }
                'p' | 'P' if flags.contains('u') && c.get(i) == Some(&'{') => {
                    let end = c[i..].iter().position(|&x| x == '}').map_or(c.len() - 1, |p| i + p);
                    out.push('\\');
                    out.push(n);
                    out.extend(&c[i..=end]);
                    i = end + 1;
                }
                // `\<` is a word boundary in Rust, a plain "<" in JavaScript
                '<' | '>' => out.push(n),
                x if x.is_ascii_punctuation() => {
                    out.push('\\');
                    out.push(x);
                }
                // an identity escape ("\a" is "a" in a non-unicode JS regex)
                x if fold && class => push_folded_member(&mut out, x),
                x if fold => push_folded(&mut out, x),
                x => push_literal(&mut out, x, class),
            }
            if class && class_open_at != usize::MAX {
                class_open_at = usize::MAX;
            }
            continue;
        }
        if class {
            match ch {
                ']' if i == class_open_at => {
                    // "[]" matches nothing, "[^]" anything
                    let negated = out.ends_with("[^");
                    out.truncate(out.len() - if negated { 2 } else { 1 });
                    out.push_str(if negated { r"(?s:.)" } else { r"[^\x00-\x{10FFFF}]" });
                    class = false;
                }
                ']' => {
                    out.push(']');
                    class = false;
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(ch);
                }
                '-' if c.get(i + 1) == Some(&'-') || out.ends_with('-') => out.push_str(r"\-"),
                // a range of plain characters: the range, then the other cases of what's in it
                lo if fold && c.get(i + 1) == Some(&'-') && c.get(i + 2).is_some_and(|&h| h != ']' && h != '\\' && h >= lo) => {
                    let hi = c[i + 2];
                    push_class_char(&mut out, lo);
                    out.push('-');
                    push_class_char(&mut out, hi);
                    for x in lo..=hi {
                        for o in equivalents(x) {
                            if !(lo..=hi).contains(&o) {
                                push_class_char(&mut out, o);
                            }
                        }
                    }
                    i += 2;
                }
                x if fold => push_folded_member(&mut out, x),
                _ => out.push(ch),
            }
            class_open_at = usize::MAX;
            i += 1;
            continue;
        }
        match ch {
            '[' => {
                class = true;
                out.push('[');
                if c.get(i + 1) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                class_open_at = i + 1;
            }
            '.' if dot_all => out.push_str("(?s:.)"),
            '.' => out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
            '(' if c.get(i + 1) == Some(&'?') => {
                let next = c.get(i + 2).copied();
                let after = c.get(i + 3).copied();
                if next == Some('=') || next == Some('!') || (next == Some('<') && (after == Some('=') || after == Some('!'))) {
                    panic!("lookaround in /{src}/ has no translation");
                }
                // "(?:" and "(?<name>" as they are: a group's name is not text to match
                let end = if next == Some('<') { c[i..].iter().position(|&x| x == '>').map_or(i + 2, |p| i + p) } else { i + 2 };
                out.extend(&c[i..=end.min(c.len() - 1)]);
                i = end + 1;
                continue;
            }
            '{' => {
                if let Some(len) = quantifier(&c[i..]) {
                    out.extend(&c[i..i + len]);
                    i += len;
                    continue;
                }
                out.push_str(r"\{");
            }
            '}' => out.push_str(r"\}"),
            x if fold && equivalents(x).len() > 1 => push_folded(&mut out, x),
            _ => out.push(ch),
        }
        i += 1;
    }
    out
}

/// JavaScript's `Canonicalize` for a non-unicode `i` pattern.
fn canonicalize(ch: char) -> char {
    let mut up = ch.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if !(ch as u32 >= 128 && (u as u32) < 128) => u,
        _ => ch,
    }
}

/// Every character that canonicalizes as `ch` does, `ch` among them.
fn equivalents(ch: char) -> Vec<char> {
    let canon = canonicalize(ch);
    if ch.is_ascii() {
        return if ch.is_ascii_alphabetic() { vec![ch.to_ascii_lowercase(), ch.to_ascii_uppercase()] } else { vec![ch] };
    }
    // neither upper- nor lower-cases to anything else: punctuation, symbols, CJK
    if canon == ch && ch.to_lowercase().eq(std::iter::once(ch)) {
        return vec![ch];
    }
    let mut out = classes().get(&canon).cloned().unwrap_or_default();
    for x in [canon, ch] {
        if !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

/// The characters that canonicalize to another, keyed by what they canonicalize to. Built on the
/// first non-ASCII letter an `i` pattern holds, by trying every character once.
fn classes() -> &'static HashMap<char, Vec<char>> {
    static CLASSES: std::sync::OnceLock<HashMap<char, Vec<char>>> = std::sync::OnceLock::new();
    CLASSES.get_or_init(|| {
        let mut map: HashMap<char, Vec<char>> = HashMap::new();
        for ch in (0..=0x10FFFF).filter_map(char::from_u32) {
            let canon = canonicalize(ch);
            if canon != ch {
                map.entry(canon).or_default().push(ch);
            }
        }
        map
    })
}

/// A letter outside a class as the class of every character it matches.
fn push_folded(out: &mut String, ch: char) {
    let all = equivalents(ch);
    if all.len() == 1 {
        return push_literal(out, ch, false);
    }
    out.push('[');
    for x in all {
        push_class_char(out, x);
    }
    out.push(']');
}

/// A letter inside a class, with the other characters it matches.
fn push_folded_member(out: &mut String, ch: char) {
    for x in equivalents(ch) {
        push_class_char(out, x);
    }
}

/// The other cases of a character already written.
fn push_others(out: &mut String, ch: char, class: bool) {
    let others: Vec<char> = equivalents(ch).into_iter().filter(|&x| x != ch).collect();
    if others.is_empty() {
        return;
    }
    if class {
        others.into_iter().for_each(|x| push_class_char(out, x));
    } else {
        // "\x{41}" becomes "(?:\x{41}|[a])"
        let start = out.rfind(r"\x{").unwrap_or(out.len());
        let first = out.split_off(start);
        out.push_str("(?:");
        out.push_str(&first);
        out.push_str("|[");
        others.into_iter().for_each(|x| push_class_char(out, x));
        out.push_str("])");
    }
}

/// One character inside a Rust class, escaped when it means something there.
fn push_class_char(out: &mut String, ch: char) {
    if matches!(ch, '\\' | '[' | ']' | '^' | '-' | '&' | '~') {
        out.push('\\');
    }
    out.push(ch);
}

fn push_class(out: &mut String, in_class: bool, set: &str, negated: bool) {
    match (in_class, negated) {
        (true, false) => out.push_str(set),
        _ => {
            out.push('[');
            if negated {
                out.push('^');
            }
            out.push_str(set);
            out.push(']');
        }
    }
}

fn push_literal(out: &mut String, ch: char, _class: bool) {
    if regex_syntax_meta(ch) {
        out.push('\\');
    }
    out.push(ch);
}

fn regex_syntax_meta(ch: char) -> bool {
    matches!(ch, '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#' | '&' | '-' | '~')
}

/// Length of a `{n}`, `{n,}` or `{n,m}` quantifier at the start of `c`, if it is one.
fn quantifier(c: &[char]) -> Option<usize> {
    let mut i = 1;
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < c.len() && c[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    if !digits(&mut i) {
        return None;
    }
    if c.get(i) == Some(&',') {
        i += 1;
        digits(&mut i);
    }
    (c.get(i) == Some(&'}')).then_some(i + 1)
}

/// `String.prototype.split(regex)`: the pieces between matches, with each match's capture
/// groups spliced in between them (an unmatched group as None).
pub fn split<'a>(s: &'a str, re: &Regex) -> Vec<Option<&'a str>> {
    if s.is_empty() {
        return if re.is_match(s) { vec![] } else { vec![Some(s)] };
    }
    let mut out = Vec::new();
    let mut p = 0;
    for caps in re.captures_iter(s) {
        let m = caps.get(0).unwrap();
        // an empty match at the start, the end, or where the last one ended splits nothing
        if m.start() == m.end() && (m.start() == 0 || m.start() >= s.len() || m.start() == p) {
            continue;
        }
        if m.start() >= s.len() {
            break;
        }
        out.push(Some(&s[p..m.start()]));
        for g in caps.iter().skip(1) {
            out.push(g.map(|g| g.as_str()));
        }
        p = m.end();
    }
    out.push(Some(&s[p..]));
    out
}

/// `split` for patterns without groups, the common case.
pub fn split_plain<'a>(s: &'a str, re: &Regex) -> Vec<&'a str> {
    split(s, re).into_iter().map(|p| p.unwrap_or("")).collect()
}

/// `s.replace(re, fn)` for a pattern without the g flag: the first match only.
pub fn replace_first(s: &str, re: &Regex, f: impl FnMut(&Captures) -> String) -> String {
    re.replacen(s, 1, f).into_owned()
}

/// `s.replace(re, fn)` with the g flag.
pub fn replace_all(s: &str, re: &Regex, f: impl FnMut(&Captures) -> String) -> String {
    re.replace_all(s, f).into_owned()
}

/// A capture group as JavaScript gives it: the text, or None when the group didn't take part.
pub fn group<'a>(c: &Captures<'a>, i: usize) -> Option<&'a str> {
    c.get(i).map(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_the_classes_javascript_means() {
        assert!(compile(r"^\s$", "").is_match("\u{feff}"));
        assert!(!compile(r"^\s$", "").is_match("\u{85}"));
        assert!(!compile(r"^\w$", "").is_match("é"));
        assert!(!compile(r"^\d$", "").is_match("٣"));
        assert!(compile(r"\bcat\b", "").is_match("a cat!"));
        assert!(!compile(r"^.$", "").is_match("\u{2028}"));
        assert!(compile(r"^[^\s\w]$", "u").is_match("❯"));
        assert!(compile(r"a{2}", "").is_match("aa"));
        assert!(compile(r"^\{x}$", "").is_match("{x}"));
        assert!(compile(r"^[\[\]]+$", "").is_match("[]"));
        assert!(compile(r"^\x1b\[[0-?]*[ -/]*[@-~]", "").is_match("\x1b[0m"));
        assert!(compile(r"esc TO", "i").is_match("Esc to"));
        assert!(compile(r"\<a\>", "").is_match("<a>"));
    }

    #[test]
    fn multiline_sees_crlf_and_a_lone_cr_as_line_ends() {
        let re = compile(r"^(\w+)[ \t]*$", "gm");
        let found: Vec<&str> = re.captures_iter("one\r\ntwo \r\nthree").map(|c| c.get(1).unwrap().as_str()).collect();
        assert_eq!(found, ["one", "two", "three"]);
        assert!(compile(r"^b$", "m").is_match("a\rb\rc"));
    }

    #[test]
    fn ignore_case_matches_as_javascript_without_u() {
        let esc = compile(r"esc to", "i");
        assert!(esc.is_match("ESC TO") && esc.is_match("Esc to"));
        // long s upper-cases to "S", but a non-ASCII letter never matches an ASCII one
        assert!(!esc.is_match("E\u{17F}c to"));
        assert!(!compile(r"^[k]$", "i").is_match("\u{212A}"));
        assert!(!compile(r"^k$", "i").is_match("\u{212A}"));
        assert!(compile(r"^[a-z]+$", "i").is_match("AbC"));
        assert!(!compile(r"^[a-z]+$", "i").is_match("\u{17F}"));
        assert!(compile(r"^[^a-c]$", "i").is_match("d") && !compile(r"^[^a-c]$", "i").is_match("B"));
        // other scripts still match their own other case, sigma all three ways
        assert!(compile(r"^é$", "i").is_match("É"));
        assert!(compile(r"^σ$", "i").is_match("ς") && compile(r"^ς$", "i").is_match("Σ"));
        assert!(compile(r"^\x41\u0062$", "i").is_match("aB"));
        assert!(compile(r"^(?<word>ab)$", "i").captures("AB").unwrap().name("word").is_some());
        assert!(compile(r"^(?:x)+$", "i").is_match("xX"));
        // with u, JavaScript folds case as Rust does
        assert!(compile(r"^k$", "iu").is_match("\u{212A}"));
    }

    #[test]
    fn split_keeps_groups() {
        let re = compile(r"(,)", "");
        assert_eq!(split("a,b", &re), vec![Some("a"), Some(","), Some("b")]);
        assert_eq!(split_plain("a  b c", &compile(r"\s{2,}", "")), vec!["a", "b c"]);
        assert_eq!(split_plain("", &compile(r"\s", "")), vec![""]);
    }
}
