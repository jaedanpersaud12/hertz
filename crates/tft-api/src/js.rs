//! The bits of JavaScript and Hono behaviour the TypeScript API relies on without saying so: how `Number()`
//! reads a string, how URLs and query strings are decoded, how `encodeURIComponent` escapes, and how
//! patches sort. Each one is written to give the same answer as the TypeScript engine, odd cases included.

use regex::Regex;
use serde_json::Value;
use std::cmp::Ordering;
use std::sync::LazyLock;

/// `Number(s)` for a string: blank is 0, hex/octal/binary prefixes, `Infinity`, otherwise a decimal or NaN.
pub fn number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    if t.is_empty() {
        return 0.0;
    }
    let radix = |digits: &str, base: u32| -> f64 {
        if digits.is_empty() {
            return f64::NAN;
        }
        let mut v = 0.0f64;
        for c in digits.chars() {
            match c.to_digit(base) {
                Some(d) => v = v * base as f64 + d as f64,
                None => return f64::NAN,
            }
        }
        v
    };
    let lower = t.get(..2).map(str::to_ascii_lowercase);
    match lower.as_deref() {
        Some("0x") => return radix(&t[2..], 16),
        Some("0o") => return radix(&t[2..], 8),
        Some("0b") => return radix(&t[2..], 2),
        _ => {}
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    static DECIMAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$").unwrap());
    if DECIMAL.is_match(t) { t.parse().unwrap_or(f64::NAN) } else { f64::NAN }
}

/// `Number(v)` for a value read from JSON.
pub fn number_of(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => number(s),
        Value::Array(xs) if xs.is_empty() => 0.0,
        Value::Array(xs) if xs.len() == 1 => match &xs[0] {
            Value::Array(_) | Value::Object(_) => f64::NAN,
            Value::Null => 0.0,
            one => number(&match one {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
        },
        _ => f64::NAN,
    }
}

/// `x.length` of a JSON value, where JavaScript has one.
pub fn length_of(v: &Value) -> Option<usize> {
    match v {
        Value::Array(xs) => Some(xs.len()),
        Value::String(s) => Some(s.encode_utf16().count()),
        _ => None,
    }
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// `decodeURI` (with `keep` = its reserved set) or `decodeURIComponent` (`keep` empty). `None` where JavaScript
/// throws a URIError.
fn decode(s: &str, keep: &[u8]) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let byte_at = |i: usize| -> Option<u8> {
        if bytes.get(i) != Some(&b'%') {
            return None;
        }
        Some(hex(*bytes.get(i + 1)?)? * 16 + hex(*bytes.get(i + 2)?)?)
    };
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let b = byte_at(i)?;
        if b < 0x80 {
            if keep.contains(&b) {
                out.extend_from_slice(&bytes[i..i + 3]);
            } else {
                out.push(b);
            }
            i += 3;
            continue;
        }
        let n = b.leading_ones() as usize;
        if !(2..=4).contains(&n) {
            return None;
        }
        let mut seq = vec![b];
        for k in 1..n {
            let next = byte_at(i + 3 * k)?;
            if next & 0xC0 != 0x80 {
                return None;
            }
            seq.push(next);
        }
        std::str::from_utf8(&seq).ok()?;
        out.extend_from_slice(&seq);
        i += 3 * n;
    }
    String::from_utf8(out).ok()
}

const URI_RESERVED: &[u8] = b";/?:@&=+$,#";

/// Hono's `tryDecode`: decode the whole string, or failing that each run of escapes that decodes on its own.
fn try_decode(s: &str, keep: &[u8]) -> String {
    if let Some(d) = decode(s, keep) {
        return d;
    }
    static RUNS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:%[0-9A-Fa-f]{2})+").unwrap());
    RUNS.replace_all(s, |c: &regex::Captures| decode(&c[0], keep).unwrap_or_else(|| c[0].to_string())).into_owned()
}

pub fn try_decode_uri(s: &str) -> String {
    try_decode(s, URI_RESERVED)
}

/// Hono's `tryDecodeURIComponent`.
pub fn try_decode_component(s: &str) -> String {
    if s.contains('%') { try_decode(s, b"") } else { s.to_string() }
}

/// Hono's `getPath`: the path, decoded with `decodeURI`, with `%25` kept as an escape.
pub fn hono_path(raw: &str) -> String {
    if !raw.contains('%') {
        return raw.to_string();
    }
    try_decode_uri(&raw.replace("%25", "%2525"))
}

/// `c.req.query(key)`: the first value for the key, names and values decoded, `+` read as a space.
pub fn query(raw: Option<&str>, key: &str) -> Option<String> {
    let raw = raw?;
    let decode = |v: &str| try_decode_component(&v.replace('+', " "));
    for part in raw.split('&') {
        let (name, value) = match part.find('=') {
            Some(i) => (&part[..i], Some(&part[i + 1..])),
            None => (part, None),
        };
        let name = decode(name);
        if name.is_empty() {
            continue;
        }
        if name == key {
            return Some(value.map(decode).unwrap_or_default());
        }
    }
    None
}

/// `encodeURIComponent`.
pub fn encode_component(s: &str) -> String {
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

/// `patchOrder` from the TypeScript API: newest patch first. A NaN result counts as equal, as `sort` treats it.
pub fn patch_order(a: &str, b: &str) -> Ordering {
    let parts = |p: &str| -> (f64, f64) {
        let mut it = p.split('.').map(number);
        (it.next().unwrap_or(f64::NAN), it.next().unwrap_or(f64::NAN))
    };
    let ((a1, a2), (b1, b2)) = (parts(a), parts(b));
    let first = b1 - a1;
    let d = if first != 0.0 && !first.is_nan() { first } else { b2 - a2 };
    d.partial_cmp(&0.0).unwrap_or(Ordering::Equal)
}

/// A JavaScript number as JSON: NaN and the infinities become null, as `JSON.stringify` writes them.
pub fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers_like_javascript() {
        let cases: &[(&str, f64)] = &[
            ("10", 10.0), ("", 0.0), ("  ", 0.0), (" 7 ", 7.0), ("1e1", 10.0), ("0x10", 16.0), ("0X1f", 31.0),
            ("0b101", 5.0), ("0o17", 15.0), ("-3", -3.0), ("5.5", 5.5), (".5", 0.5), ("5.", 5.0), ("+4", 4.0),
            ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY),
        ];
        for (s, want) in cases {
            assert_eq!(number(s), *want, "{s:?}");
        }
        for s in ["abc", "inf", "infinity", "NaN", "1,2", "-0x10", "0x", "1e", "1_0", "e5", "."] {
            assert!(number(s).is_nan(), "{s:?}");
        }
    }

    #[test]
    fn decodes_like_hono() {
        assert_eq!(hono_path("/api/comps/A%7CB"), "/api/comps/A|B");
        assert_eq!(hono_path("/a%2Fb"), "/a%2Fb"); // decodeURI keeps reserved escapes
        assert_eq!(hono_path("/a%2520b"), "/a%2520b");
        assert_eq!(hono_path("/x/%E0%A4%A"), "/x/%E0%A4%A");
        assert_eq!(hono_path("/%C3%A9/%ZZ/%41"), "/é/%ZZ/A");
        assert_eq!(try_decode_component("a%2Fb%7c"), "a/b|");
        assert_eq!(try_decode_component("%E0%A4%A"), "%E0%A4%A");
        assert_eq!(try_decode_component("%C0%80"), "%C0%80"); // overlong
        assert_eq!(query(Some("patch=1&patch=2"), "patch").as_deref(), Some("1"));
        assert_eq!(query(Some("pat%63h=3&patch=2"), "patch").as_deref(), Some("3"));
        assert_eq!(query(Some("min"), "min").as_deref(), Some(""));
        assert_eq!(query(Some("b=PLATINUM+&c=x%2By"), "b").as_deref(), Some("PLATINUM "));
        assert_eq!(query(Some("b=PLATINUM+&c=x%2By"), "c").as_deref(), Some("x+y"));
        assert_eq!(query(Some("a=1=2"), "a").as_deref(), Some("1=2"));
        assert_eq!(query(Some("a=1"), "b"), None);
        assert_eq!(query(None, "b"), None);
        assert_eq!(encode_component("A|b c/é_(x)"), "A%7Cb%20c%2F%C3%A9_(x)");
    }

    #[test]
    fn sorts_patches_newest_first() {
        let mut ps = vec!["16.21", "99.3", "16.22", "99.10", "16.3"];
        ps.sort_by(|a, b| patch_order(a, b));
        assert_eq!(ps, ["99.10", "99.3", "16.22", "16.21", "16.3"]);
        assert_eq!(patch_order("unknown", "16.1"), Ordering::Equal);
    }
}
