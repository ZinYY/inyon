//! The two encodings control mode needs: `%output` data coming out, command arguments going in.

/// Decodes the data of an `%output` line. tmux writes bytes below 0x20 and the backslash as a
/// backslash and three octal digits (`\015`, `\134`); every other byte, UTF-8 included, is raw.
/// psmux writes a backslash as `\\`, so that form is decoded too.
pub fn unescape_output(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == b'\\' {
            if let [d1 @ b'0'..=b'3', d2 @ b'0'..=b'7', d3 @ b'0'..=b'7', ..] = data[i + 1..] {
                out.push((d1 - b'0') * 64 + (d2 - b'0') * 8 + (d3 - b'0'));
                i += 4;
                continue;
            }
            if data.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

/// Quotes one argument for tmux's command parser (3.0 and later), so any text survives a single
/// control-mode line: double quotes, with the characters the parser treats specially escaped and
/// newlines written as `\n` (a raw newline would end the command).
pub fn quote(arg: &str) -> String {
    let mut s = String::with_capacity(arg.len() + 2);
    s.push('"');
    for c in arg.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            // $NAME expands inside double quotes
            '$' => s.push_str("\\$"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            '\x1b' => s.push_str("\\e"),
            '\x07' => s.push_str("\\a"),
            '\x08' => s.push_str("\\b"),
            '\x0c' => s.push_str("\\f"),
            '\x0b' => s.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\x7f' => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_octal_and_keeps_utf8() {
        assert_eq!(unescape_output(br"a\015\012b\134c"), b"a\r\nb\\c");
        assert_eq!(unescape_output("\\033[31mrød\\033[0m".as_bytes()), "\x1b[31mrød\x1b[0m".as_bytes());
        assert_eq!(unescape_output(br"\000\037"), b"\x00\x1f");
    }

    #[test]
    fn leaves_broken_escapes_alone() {
        // a truncated or non-octal escape is kept as written rather than guessed at
        assert_eq!(unescape_output(br"x\01"), br"x\01");
        assert_eq!(unescape_output(br"x\9"), br"x\9");
        assert_eq!(unescape_output(br"x\"), br"x\");
        assert_eq!(unescape_output(br"\477"), br"\477");
    }

    #[test]
    fn reads_psmux_doubled_backslash() {
        assert_eq!(unescape_output(br"C:\\Users\015"), b"C:\\Users\r");
    }

    #[test]
    fn quotes_specials() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("a \"b\" $HOME \\ ;"), r#""a \"b\" \$HOME \\ ;""#);
        assert_eq!(quote("l1\nl2\tx\x1b[A"), r#""l1\nl2\tx\e[A""#);
        assert_eq!(quote("\x01\x7f"), r#""\u0001\u007f""#);
    }
}
