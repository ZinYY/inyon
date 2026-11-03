//! The few JavaScript behaviours the ported code leans on: `Date.parse`, `Math.round`,
//! `toFixed`, `Number()`, `encodeURIComponent`, JavaScript's white space, and string lengths in
//! UTF-16 code units. The date parser and the number helpers are the ones in inyon-core's `js`
//! module, copied so the app doesn't pull in the hub core for them.

/// What `\s` matches in a JavaScript regex and what `trim()` removes.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `s.trim()`
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.trimEnd()`
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `s.replace(/\s+/g, ' ')`
pub fn squash_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if is_space(c) {
            if !gap {
                out.push(' ');
            }
            gap = true;
        } else {
            out.push(c);
            gap = false;
        }
    }
    out
}

/// `s.length`: UTF-16 code units.
pub fn len16(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)`, counted in UTF-16 code units. A character that would be cut in half is left
/// out whole, where JavaScript would keep half of it.
pub fn cut16(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// `split(/\r?\n|\r| | /)`: the lines a regex's `^` and `$` see with the `m` flag.
pub fn lines(s: &str) -> impl Iterator<Item = &str> {
    s.split(['\n', '\r', '\u{2028}', '\u{2029}'])
}

/// `Math.round`: halves go up, toward +∞ (Rust's `round` takes them away from zero).
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// `x.toFixed(digits)`: an exact half rounds up, where Rust's formatting rounds it to even.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() {
        return format!("{x}");
    }
    // the exact decimal expansion, long enough to see whether the cut lands on a half
    let wide = format!("{:.*}", digits + 30, x.abs());
    let (int, frac) = wide.split_once('.').unwrap_or((&wide, ""));
    let mut kept: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let int_len = kept.len() - digits;
    let mut out = String::from_utf8(kept[..int_len].to_vec()).unwrap_or_default();
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&kept[int_len..]).unwrap_or_default());
    }
    let zero = out.bytes().all(|b| b == b'0' || b == b'.');
    if x.is_sign_negative() && !zero {
        out.insert(0, '-');
    }
    out
}

/// `Number(s)` for the decimal strings the app reads: white space around it is ignored, "" is 0,
/// anything that isn't a decimal number is NaN.
pub fn to_number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Rust takes "inf" and "nan"; JavaScript takes only digits, one dot and an exponent
    let b = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (mant, exp) = match b.find(['e', 'E']) {
        Some(i) => (&b[..i], Some(&b[i + 1..])),
        None => (b, None),
    };
    let mut parts = mant.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let mant_ok = digits(int) && frac.is_none_or(digits) && !(int.is_empty() && frac.is_none_or(str::is_empty));
    let exp_ok = exp.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    if !(mant_ok && exp_ok) {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
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

/// `Date.parse(s)` for ISO 8601 timestamps: milliseconds since the epoch, or NaN. A date and time
/// with no offset is read as UTC, where JavaScript reads it in local time; agents write offsets.
pub fn parse_date(s: &str) -> f64 {
    parse_iso(trim(s)).unwrap_or(f64::NAN)
}

fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, n: usize| -> Option<i64> {
        let part = b.get(*i..*i + n)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    let year = match b.first()? {
        b'+' | b'-' => {
            let neg = b[0] == b'-';
            i = 1;
            let y = num(&mut i, 6)?;
            if neg && y == 0 {
                return None;
            }
            if neg {
                -y
            } else {
                y
            }
        }
        _ => num(&mut i, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = num(&mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = num(&mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let (mut h, mut mi, mut sec, mut ms) = (0, 0, 0, 0.0);
    let mut offset_min = 0;
    if i < b.len() {
        if b[i] != b'T' && b[i] != b't' && b[i] != b' ' {
            return None;
        }
        i += 1;
        h = num(&mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        mi = num(&mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
            sec = num(&mut i, 2)?;
            if b.get(i) == Some(&b'.') || b.get(i) == Some(&b',') {
                i += 1;
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                // JavaScript keeps whole milliseconds
                let three: String = s[start..i].chars().chain("000".chars()).take(3).collect();
                ms = three.parse::<f64>().ok()?;
            }
        }
        if h > 24 || mi > 59 || sec > 59 || (h == 24 && (mi > 0 || sec > 0 || ms > 0.0)) {
            return None;
        }
        match b.get(i) {
            None => {}
            Some(b'Z') | Some(b'z') => i += 1,
            Some(b'+') | Some(b'-') => {
                let sign = if b[i] == b'-' { -1 } else { 1 };
                i += 1;
                let oh = num(&mut i, 2)?;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                let om = num(&mut i, 2)?;
                if oh > 23 || om > 59 {
                    return None;
                }
                offset_min = sign * (oh * 60 + om);
            }
            _ => return None,
        }
        if i != b.len() {
            return None;
        }
    }
    let days = days_from_civil(year, month, day);
    let t = (days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60) as f64 * 1000.0 + ms;
    (t.abs() <= 8.64e15).then_some(t)
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A moment on the clock of a place `offset_min` minutes east of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    /// 0 is Sunday, as `getDay()` counts
    pub weekday: i64,
    pub hour: i64,
    pub minute: i64,
}

pub fn local_time(ms: f64, offset_min: i32) -> LocalTime {
    let t = (ms / 1000.0).floor() as i64 + offset_min as i64 * 60;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    LocalTime { year, month, day, weekday: (days + 4).rem_euclid(7), hour: secs / 3600, minute: secs % 3600 / 60 }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

impl LocalTime {
    /// `toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' })`: "3:05 PM"
    pub fn clock(&self) -> String {
        let h = match self.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h}:{:02} {}", self.minute, if self.hour < 12 { "AM" } else { "PM" })
    }

    /// `toLocaleDateString('en-US', { month: 'short', day: 'numeric' })`: "Oct 8"
    pub fn month_day(&self) -> String {
        format!("{} {}", MONTHS[(self.month - 1) as usize], self.day)
    }

    /// `toLocaleString('en-US', { weekday: 'short', hour: 'numeric', minute: '2-digit' })`: "Sat 3:05 PM"
    pub fn weekday_clock(&self) -> String {
        format!("{} {}", WEEKDAYS[self.weekday as usize], self.clock())
    }

    pub fn same_day(&self, other: &LocalTime) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_halves_up_like_javascript() {
        assert_eq!(to_fixed(100.5, 0), "101");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00"); // 1.005 is a little under in binary
        assert_eq!(to_fixed(9.99, 1), "10.0");
        assert_eq!(to_fixed(3.456, 2), "3.46");
        assert_eq!(to_fixed(-0.001, 2), "0.00");
        assert_eq!(to_fixed(-1.5, 0), "-2");
    }

    #[test]
    fn dates_parse_as_javascript_reads_iso() {
        assert_eq!(parse_date("2026-10-11T12:00:00.000Z"), 1_791_720_000_000.0);
        assert_eq!(parse_date("2026-10-11T14:00:00+02:00"), 1_791_720_000_000.0);
        assert!(parse_date("not a date").is_nan());
        assert!(parse_date("").is_nan());
    }

    #[test]
    fn local_times_read_on_the_given_clock() {
        let t = local_time(parse_date("2026-10-11T12:05:00Z"), 0);
        assert_eq!((t.clock(), t.month_day(), t.weekday_clock()), ("12:05 PM".into(), "Oct 11".into(), "Sun 12:05 PM".into()));
        let t = local_time(parse_date("2026-10-11T23:30:00Z"), 120);
        assert_eq!((t.clock(), t.month_day()), ("1:30 AM".into(), "Oct 12".into()));
    }

    #[test]
    fn strings_count_utf16() {
        assert_eq!(len16("a😀b"), 4);
        assert_eq!(cut16("a😀b", 3), "a😀");
        assert_eq!(cut16("a😀b", 2), "a");
        assert_eq!(squash_spaces("  a \n\t b  "), " a b ");
        assert_eq!(to_number(" 4 "), 4.0);
        assert!(to_number("x").is_nan());
        assert_eq!(encode_uri_component("my box/a"), "my%20box%2Fa");
    }
}
