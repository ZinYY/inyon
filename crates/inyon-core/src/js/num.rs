//! Numbers as JavaScript reads, rounds and prints them.

use super::text::JsStr;

/// `Number(s)`: white space around it is ignored, "" is 0, "0x1f" is hex, anything else that
/// isn't a decimal number is NaN.
pub fn to_number(s: &str) -> f64 {
    let t = s.jtrim();
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if rest.is_empty() || !rest.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return rest.chars().fold(0.0, |n, c| n * radix as f64 + c.to_digit(radix).unwrap() as f64);
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Rust takes "inf", "nan" and "1_0"-free decimals; JavaScript takes only digits, one dot and an exponent
    let ok = {
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
        mant_ok && exp_ok
    };
    if !ok {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
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

/// `parseInt(s, radix)` for the hex pairs the readers decode.
pub fn parse_int(s: &str, radix: u32) -> f64 {
    let t = s.jtrim_start();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = t.chars().take_while(|c| c.is_digit(radix)).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    let v = digits.chars().fold(0.0, |n, c| n * radix as f64 + c.to_digit(radix).unwrap() as f64);
    if neg {
        -v
    } else {
        v
    }
}

/// `String(x)` for a number: the shortest digits that read back as `x`, in plain notation from
/// 1e-7 up to 1e21 and in exponent notation ("1e+21", "1.5e-7") outside that.
pub fn fmt_number(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x < 0.0 {
        return format!("-{}", fmt_number(-x));
    }
    // Rust's {:e} gives the shortest round-trip digits: "1.2345e-7"
    let e = format!("{x:e}");
    let (mant, exp) = e.split_once('e').unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap() + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let e = (n - 1).abs();
        if k == 1 {
            format!("{digits}e{sign}{e}")
        } else {
            format!("{}.{}e{sign}{e}", &digits[..1], &digits[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_numbers_like_javascript() {
        for (x, s) in
            [(1.0, "1"), (0.1, "0.1"), (1e21, "1e+21"), (1.5e-7, "1.5e-7"), (123456.789, "123456.789"), (1e-6, "0.000001"), (-2.5, "-2.5"), (1e20, "100000000000000000000")]
        {
            assert_eq!(fmt_number(x), s);
        }
    }

    #[test]
    fn reads_numbers_like_javascript() {
        assert_eq!(to_number(" 12 "), 12.0);
        assert_eq!(to_number(""), 0.0);
        assert!(to_number("12px").is_nan());
        assert!(to_number("inf").is_nan());
        assert_eq!(to_number("0x1f"), 31.0);
        assert_eq!(to_number(".5"), 0.5);
        assert!(to_number(".").is_nan());
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
    }
}
