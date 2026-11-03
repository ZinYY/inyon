//! JSON values read the way the TS readers read `any`: `a?.b` on anything, JavaScript's
//! truthiness, `typeof` checks and `String(v)`, plus `JSON.stringify` byte for byte (the Codex
//! reader hashes it).

use serde_json::{Map, Value};

use super::num::fmt_number;

/// `JSON.stringify(v)`: no spaces, keys in insertion order except integer keys, which come first
/// in ascending order (JavaScript objects keep them that way), and numbers as JavaScript prints them.
pub fn stringify(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v);
    out
}

fn write(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&fmt_number(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => quote(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, x);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in ordered(o).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                quote(out, k);
                out.push(':');
                write(out, x);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(v, null, 2)`: the same, one member per line, indented two spaces a level.
pub fn stringify_pretty(v: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, v, 0);
    out
}

fn write_pretty(out: &mut String, v: &Value, depth: usize) {
    let pad = |out: &mut String, d: usize| {
        out.push('\n');
        for _ in 0..d {
            out.push_str("  ");
        }
    };
    match v {
        Value::Array(a) if !a.is_empty() => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_pretty(out, x, depth + 1);
            }
            pad(out, depth);
            out.push(']');
        }
        Value::Object(o) if !o.is_empty() => {
            out.push('{');
            for (i, (k, x)) in ordered(o).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                quote(out, k);
                out.push_str(": ");
                write_pretty(out, x, depth + 1);
            }
            pad(out, depth);
            out.push('}');
        }
        _ => write(out, v),
    }
}

/// `{ ...v }`: an object's own members, an array's or a string's by index, nothing for the rest.
pub fn spread(v: Option<&Value>) -> Map<String, Value> {
    match v {
        Some(Value::Object(m)) => m.clone(),
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, x)| (i.to_string(), x.clone())).collect(),
        Some(Value::String(s)) => s.encode_utf16().enumerate().map(|(i, u)| (i.to_string(), Value::String(String::from_utf16_lossy(&[u])))).collect(),
        _ => Map::new(),
    }
}

/// An array index key ("0" up to 2^32 - 2, no leading zeros).
fn index_key(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) || !k.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    k.parse::<u32>().ok().filter(|n| *n < u32::MAX)
}

/// A map as a JavaScript object built by assigning its keys in this order would hold them:
/// integer-like keys first, ascending, then the rest as they came.
pub fn js_object_order<V>(m: indexmap::IndexMap<String, V>) -> indexmap::IndexMap<String, V> {
    if !m.keys().any(|k| index_key(k).is_some()) {
        return m;
    }
    let (mut ints, rest): (Vec<_>, Vec<_>) = m.into_iter().partition(|(k, _)| index_key(k).is_some());
    ints.sort_by_key(|(k, _)| index_key(k));
    ints.into_iter().chain(rest).collect()
}

/// An object's keys in the order JavaScript enumerates them.
pub fn ordered(o: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut ints: Vec<(u32, (&String, &Value))> = Vec::new();
    let mut rest = Vec::new();
    for (k, v) in o {
        match index_key(k) {
            Some(n) => ints.push((n, (k, v))),
            None => rest.push((k, v)),
        }
    }
    ints.sort_by_key(|(n, _)| *n);
    ints.into_iter().map(|(_, kv)| kv).chain(rest).collect()
}

fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// How deeply arrays and objects may nest in what [`parse`] reads. `JSON.parse` takes any depth
/// (Bun reads a million); serde_json and dropping a `Value` both recurse, so past this the text
/// reads as not JSON instead of running out of stack. 256 levels take about 256 KB of stack in a
/// release build and 1 MB in a debug one, which leaves room on a 2 MB worker thread.
pub const MAX_DEPTH: usize = 256;

/// `JSON.parse(s)`, or None where it throws. Three things JavaScript takes that serde_json alone
/// refuses are read the way the TS goes on to see them:
/// - a `\uD83D` escape without the other half of its pair (a transcript line cut in the middle of
///   an emoji has one): a Rust string can't hold half a pair, so it reads as U+FFFD;
/// - a number too big for a double (`1e400`): JavaScript reads Infinity, which a JSON value can't
///   hold, so it reads as null, which is what `JSON.stringify` writes for it;
/// - nesting deeper than serde_json's 128 levels, up to [`MAX_DEPTH`].
pub fn parse(s: &str) -> Option<Value> {
    match serde_json::from_str(s) {
        Ok(v) => Some(v),
        Err(_) => parse_lenient(s),
    }
}

/// The second try, for text serde_json refused.
fn parse_lenient(s: &str) -> Option<Value> {
    if depth(s) > MAX_DEPTH {
        return None;
    }
    let s = if s.contains("\\u") { lone_surrogates(s) } else { s.to_owned() };
    let s = infinities(&s);
    let mut de = serde_json::Deserializer::from_str(&s);
    de.disable_recursion_limit();
    let v = serde::Deserialize::deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(v)
}

/// The deepest any array or object in the text nests, strings skipped.
fn depth(s: &str) -> usize {
    let (mut depth, mut max, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for b in s.bytes() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// The text with every number outside a string that overflows a double written as null.
fn infinities(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut last, mut in_string, mut escaped) = (0, 0, false, false);
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if c == b'-' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || matches!(b[i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                i += 1;
            }
            if s[start..i].parse::<f64>().is_ok_and(f64::is_infinite) {
                out.push_str(&s[last..start]);
                out.push_str("null");
                last = i;
            }
            continue;
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

/// The text with every `\uXXXX` escape of an unpaired surrogate written as `\uFFFD`.
fn lone_surrogates(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |i: usize| -> Option<u16> { s.get(i..i + 4).and_then(|h| u16::from_str_radix(h, 16).ok()) };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            if b[i + 1] == b'u' {
                if let Some(u) = hex(i + 2) {
                    let paired_high =
                        (0xd800..0xdc00).contains(&u) && b.get(i + 6) == Some(&b'\\') && b.get(i + 7) == Some(&b'u') && hex(i + 8).is_some_and(|l| (0xdc00..0xe000).contains(&l));
                    if paired_high {
                        i += 12;
                        continue;
                    }
                    if (0xd800..0xe000).contains(&u) {
                        out.push_str(&s[last..i]);
                        out.push_str("\\uFFFD");
                        i += 6;
                        last = i;
                        continue;
                    }
                }
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

/// A value the TS reads as `any`: `None` is `undefined`.
#[allow(clippy::wrong_self_convention)] // the receiver is an Option<&Value>, which is Copy
pub trait Js<'a> {
    /// `v?.[key]`
    fn at(self, key: &str) -> Option<&'a Value>;
    /// `v?.[i]`
    fn idx(self, i: usize) -> Option<&'a Value>;
    /// `v ?? other`: undefined and null fall through. A null `other` stays null, so a chain ending
    /// in a default goes through [`Js::string_or`] or [`Js::string_or_empty`], never
    /// `.map(value_string)`, which would print it as "null".
    fn or(self, other: Option<&'a Value>) -> Option<&'a Value>;
    /// neither undefined nor null
    fn present(self) -> Option<&'a Value>;
    /// JavaScript truthiness
    fn truthy(self) -> bool;
    /// `typeof v === 'string'`
    fn str(self) -> Option<&'a str>;
    /// a string that isn't empty (`typeof v === 'string' && v`, or `v` as a truthy string)
    fn nonempty(self) -> Option<&'a str>;
    /// `typeof v === 'number'` (finite: JSON has no other kind)
    fn num(self) -> Option<f64>;
    /// `Array.isArray(v)`
    fn arr(self) -> Option<&'a Vec<Value>>;
    /// a plain object
    fn obj(self) -> Option<&'a Map<String, Value>>;
    /// `typeof v === 'object' && v !== null` (arrays included)
    fn is_object(self) -> bool;
    /// `v === true`
    fn is_true(self) -> bool;
    /// `String(v)`, with undefined as "undefined"
    fn to_js_string(self) -> String;
    /// `String(v ?? '')`
    fn string_or_empty(self) -> String;
    /// `String(v ?? default)`: the default wins over undefined and null alike
    fn string_or(self, default: &str) -> String;
    /// the value as it would be copied into another object (`{ x: v }`), dropped when undefined
    fn cloned_value(self) -> Option<Value>;
}

impl<'a> Js<'a> for Option<&'a Value> {
    fn at(self, key: &str) -> Option<&'a Value> {
        match self? {
            Value::Object(o) => o.get(key),
            Value::Array(a) => index_key(key).and_then(|i| a.get(i as usize)),
            _ => None,
        }
    }
    fn idx(self, i: usize) -> Option<&'a Value> {
        match self? {
            Value::Array(a) => a.get(i),
            Value::Object(o) => o.get(&i.to_string()),
            _ => None,
        }
    }
    fn or(self, other: Option<&'a Value>) -> Option<&'a Value> {
        self.present().or(other)
    }
    fn present(self) -> Option<&'a Value> {
        self.filter(|v| !v.is_null())
    }
    fn truthy(self) -> bool {
        match self {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
            Some(Value::String(s)) => !s.is_empty(),
            Some(_) => true,
        }
    }
    fn str(self) -> Option<&'a str> {
        self.and_then(Value::as_str)
    }
    fn nonempty(self) -> Option<&'a str> {
        self.str().filter(|s| !s.is_empty())
    }
    fn num(self) -> Option<f64> {
        self.and_then(Value::as_f64)
    }
    fn arr(self) -> Option<&'a Vec<Value>> {
        self.and_then(Value::as_array)
    }
    fn obj(self) -> Option<&'a Map<String, Value>> {
        self.and_then(Value::as_object)
    }
    fn is_object(self) -> bool {
        matches!(self, Some(Value::Object(_)) | Some(Value::Array(_)))
    }
    fn is_true(self) -> bool {
        matches!(self, Some(Value::Bool(true)))
    }
    fn to_js_string(self) -> String {
        match self {
            None => "undefined".into(),
            Some(v) => value_string(v),
        }
    }
    fn string_or_empty(self) -> String {
        match self.present() {
            None => String::new(),
            Some(v) => value_string(v),
        }
    }
    fn string_or(self, default: &str) -> String {
        self.present().map_or_else(|| default.to_owned(), value_string)
    }
    fn cloned_value(self) -> Option<Value> {
        self.cloned()
    }
}

/// `String(v)` for a JSON value.
pub fn value_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => fmt_number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| if x.is_null() { String::new() } else { value_string(x) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `a === b` for two JSON values: numbers by value (1 and 1.0 are one number), and two objects or
/// arrays never, as two parsed ones are never the same object.
pub fn strict_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

/// `Number(v)` for a JSON value (`+v`, and what `v >= 1000` compares).
pub fn value_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => super::num::to_number(s),
        Value::Array(_) => super::num::to_number(&value_string(v)),
        Value::Object(_) => f64::NAN,
    }
}

/// A JavaScript `Map` key as a string: a string is itself, anything else (undefined, null, a
/// number) gets a key no string read from a record holds, so `map.get('')` misses a value set
/// under undefined as it does in JavaScript. [`key_value`] turns it back for saving.
pub fn map_key(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        None => "\0undefined".into(),
        Some(v) => format!("\0{}", stringify(v)),
    }
}

/// The key as `JSON.stringify([...map])` writes it: undefined as null.
pub fn key_value(k: &str) -> Value {
    match k.strip_prefix('\0') {
        Some("undefined") => Value::Null,
        Some(json) => parse(json).unwrap_or_else(|| Value::String(k.to_owned())),
        None => Value::String(k.to_owned()),
    }
}

/// A key back from what [`key_value`] wrote.
pub fn key_of(v: &Value) -> String {
    map_key(Some(v))
}

/// A JSON number from an f64, written as JavaScript would (whole values as integers).
pub fn number(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        Value::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
    }
}

/// An object literal: `obj([("command", v), ...])`, leaving out the entries that are undefined
/// (None), as `JSON.stringify` and the wire would.
pub fn obj<const N: usize>(entries: [(&str, Option<Value>); N]) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

/// A string as a JSON value.
pub fn s(v: impl Into<String>) -> Value {
    Value::String(v.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_deep_nesting_and_overflowing_numbers_as_javascript_goes_on_to_see_them() {
        let nested = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(&nested(250)).is_some());
        assert!(parse(&format!("{{\"a\":{}}}", nested(200))).is_some());
        assert!(parse(&nested(MAX_DEPTH + 1)).is_none());
        // far past the limit: refused, on a worker-sized stack, without running out of it
        let deep = nested(1_000_000);
        let got = std::thread::Builder::new().stack_size(2 << 20).spawn(move || parse(&deep).is_none()).unwrap().join().unwrap();
        assert!(got);
        assert_eq!(
            parse(r#"{"a":1e400,"b":-1e400,"c":1e-400,"d":"1e400","e":[2e308]}"#).unwrap(),
            serde_json::json!({ "a": null, "b": null, "c": 0.0, "d": "1e400", "e": [null] })
        );
        assert_eq!(parse("[1e400] x"), None);
        assert_eq!(parse(r#"["\ud83d", 1e999]"#).unwrap(), serde_json::json!(["\u{fffd}", null]));
    }

    #[test]
    fn half_a_surrogate_pair_still_parses() {
        assert_eq!(parse(r#"{"t":"a\ud83d"}"#).unwrap()["t"], "a\u{fffd}");
        assert_eq!(parse(r#"{"t":"\ud83d\ude42 \\ud83d"}"#).unwrap()["t"], "🙂 \\ud83d");
        assert_eq!(parse("{"), None);
    }

    #[test]
    fn pretty_prints_like_javascript() {
        let v: Value = serde_json::from_str(r#"{"b":[],"a":{},"2":[1,{"x":"y"}],"s":"q\""}"#).unwrap();
        assert_eq!(stringify_pretty(&v), "{\n  \"2\": [\n    1,\n    {\n      \"x\": \"y\"\n    }\n  ],\n  \"b\": [],\n  \"a\": {},\n  \"s\": \"q\\\"\"\n}");
        assert_eq!(stringify_pretty(&Value::from(1.5)), "1.5");
    }

    #[test]
    fn stringifies_like_javascript() {
        let v: Value = serde_json::from_str(r#"{"b":1,"2":2,"a":[1.5e-7,1e21,"\u001f\u2028"],"1":0,"c":3.0}"#).unwrap();
        assert_eq!(stringify(&v), "{\"1\":0,\"2\":2,\"b\":1,\"a\":[1.5e-7,1e+21,\"\\u001f\u{2028}\"],\"c\":3}");
    }
}
