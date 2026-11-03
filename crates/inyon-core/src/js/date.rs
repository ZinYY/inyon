//! `Date.parse` and `toISOString` for the timestamps agents write (ISO 8601, mostly with a Z).
//!
//! One difference from JavaScript: a date and time with no offset is read as UTC here, where
//! JavaScript reads it in the machine's local time zone. The core has no time zone (it runs as
//! wasm too), and every agent the readers know writes its times with an offset.

/// Milliseconds since the epoch, or NaN when JavaScript's `Date.parse` would give NaN.
pub fn parse(s: &str) -> f64 {
    parse_iso(s.trim()).unwrap_or(f64::NAN)
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
                let frac = &s[start..i];
                let three: String = frac.chars().chain("000".chars()).take(3).collect();
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

/// `new Date(ms).toISOString()`, or None where JavaScript throws (an invalid date).
pub fn to_iso(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    let ms = ms.trunc() as i64;
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    let year = if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y < 0 {
        format!("-{:06}", -y)
    } else {
        format!("+{y:06}")
    };
    Some(format!("{year}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z"))
}

/// `new Date(v)` for a JSON value (a number of ms, or a string to parse) → its time, or NaN.
pub fn time_of(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        serde_json::Value::String(s) => parse(s),
        serde_json::Value::Bool(b) => *b as i64 as f64,
        serde_json::Value::Null => 0.0,
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_iso_times() {
        assert_eq!(parse("2026-10-14T12:00:00.000Z"), 1_791_979_200_000.0);
        assert_eq!(parse("2026-10-14T12:00:00Z"), 1_791_979_200_000.0);
        assert_eq!(parse("2026-10-14T14:00:00+02:00"), 1_791_979_200_000.0);
        assert_eq!(parse("2026-10-14"), 1_791_936_000_000.0);
        assert!(parse("").is_nan());
        assert!(parse("yesterday").is_nan());
        assert_eq!(to_iso(1_791_979_200_123.9).unwrap(), "2026-10-14T12:00:00.123Z");
        assert_eq!(to_iso(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert!(to_iso(f64::NAN).is_none());
    }
}
