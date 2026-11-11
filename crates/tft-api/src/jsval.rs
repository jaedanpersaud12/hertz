//! A parsed JSON value that behaves like the JavaScript value `JSON.parse` gives: object keys keep their order
//! (a repeated key keeps its first place and its last value), every number is a double, and `stringify` writes
//! exactly what `JSON.stringify` writes. The write and summoner routes store request bodies and Riot answers the
//! way the TypeScript API does (`JSON.stringify(JSON.parse(text))`), so the stored text matches byte for byte.

use crate::js::num;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsValue>),
    Obj(Vec<(String, JsValue)>),
}

impl<'de> Deserialize<'de> for JsValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JsValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<JsValue, E> {
                Ok(JsValue::Null)
            }
            fn visit_none<E>(self) -> Result<JsValue, E> {
                Ok(JsValue::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<JsValue, D::Error> {
                JsValue::deserialize(d)
            }
            fn visit_bool<E>(self, b: bool) -> Result<JsValue, E> {
                Ok(JsValue::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<JsValue, E> {
                Ok(JsValue::Num(n as f64))
            }
            fn visit_u64<E>(self, n: u64) -> Result<JsValue, E> {
                Ok(JsValue::Num(n as f64))
            }
            fn visit_f64<E>(self, n: f64) -> Result<JsValue, E> {
                Ok(JsValue::Num(n))
            }
            fn visit_str<E>(self, s: &str) -> Result<JsValue, E> {
                Ok(JsValue::Str(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<JsValue, E> {
                Ok(JsValue::Str(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<JsValue, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = seq.next_element()? {
                    out.push(v);
                }
                Ok(JsValue::Arr(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsValue, A::Error> {
                let mut out: Vec<(String, JsValue)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, JsValue>()? {
                    match out.iter_mut().find(|(have, _)| *have == k) {
                        Some(slot) => slot.1 = v,
                        None => out.push((k, v)),
                    }
                }
                Ok(JsValue::Obj(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// `JSON.parse` of a request body as `Request.json()` reads it: UTF-8 with bad bytes replaced, a leading BOM dropped.
pub fn parse_body(bytes: &[u8]) -> Option<JsValue> {
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
}

pub fn parse(text: &str) -> anyhow::Result<JsValue> {
    Ok(serde_json::from_str(text)?)
}

/// `Number.prototype.toString()`: the shortest digits that read back as the same double, in JavaScript's layout.
pub fn number_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return "0".into();
    }
    let sci = format!("{:e}", x.abs()); // shortest round-trip digits, e.g. "1.5e-7"
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap() + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let rest = if k > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        let e = n - 1;
        format!("{}{rest}e{}{}", &digits[..1], if e < 0 { '-' } else { '+' }, e.abs())
    };
    if x < 0.0 { format!("-{body}") } else { body }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

impl JsValue {
    /// `JSON.stringify(value)`.
    pub fn stringify(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
    fn write(&self, out: &mut String) {
        match self {
            JsValue::Null => out.push_str("null"),
            JsValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsValue::Num(x) if x.is_finite() => out.push_str(&number_string(*x)),
            JsValue::Num(_) => out.push_str("null"),
            JsValue::Str(s) => quote(s, out),
            JsValue::Arr(xs) => {
                out.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            JsValue::Obj(kv) => {
                out.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// The value as a response field (key order does not matter there).
    pub fn to_json(&self) -> Value {
        match self {
            JsValue::Null => Value::Null,
            JsValue::Bool(b) => Value::Bool(*b),
            JsValue::Num(x) if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 => Value::from(*x as i64),
            JsValue::Num(x) => num(*x),
            JsValue::Str(s) => Value::String(s.clone()),
            JsValue::Arr(xs) => Value::Array(xs.iter().map(JsValue::to_json).collect()),
            JsValue::Obj(kv) => Value::Object(kv.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
        }
    }

    /// `value.key` for an object; `undefined` (None) for anything else, as for the values JSON can hold.
    pub fn get(&self, key: &str) -> Option<&JsValue> {
        match self {
            JsValue::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsValue]> {
        match self {
            JsValue::Arr(xs) => Some(xs),
            _ => None,
        }
    }

    /// `Boolean(value)`.
    pub fn truthy(&self) -> bool {
        match self {
            JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Num(x) => *x != 0.0 && !x.is_nan(),
            JsValue::Str(s) => !s.is_empty(),
            JsValue::Arr(_) | JsValue::Obj(_) => true,
        }
    }

    /// `Number(value)`.
    pub fn to_number(&self) -> f64 {
        match self {
            JsValue::Null => 0.0,
            JsValue::Bool(b) => f64::from(u8::from(*b)),
            JsValue::Num(x) => *x,
            JsValue::Str(s) => crate::js::number(s),
            JsValue::Arr(_) => crate::js::number(&self.to_js_string()),
            JsValue::Obj(_) => f64::NAN,
        }
    }

    /// `String(value)`.
    pub fn to_js_string(&self) -> String {
        match self {
            JsValue::Null => "null".into(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Num(x) => number_string(*x),
            JsValue::Str(s) => s.clone(),
            // Array.prototype.join: null and undefined entries are blank.
            JsValue::Arr(xs) => xs
                .iter()
                .map(|x| if *x == JsValue::Null { String::new() } else { x.to_js_string() })
                .collect::<Vec<_>>()
                .join(","),
            JsValue::Obj(_) => "[object Object]".into(),
        }
    }
}

/// `String(v)` where `v` may be `undefined`.
pub fn js_string(v: Option<&JsValue>) -> String {
    v.map(JsValue::to_js_string).unwrap_or_else(|| "undefined".into())
}

/// `Number(v)` where `v` may be `undefined`.
pub fn to_number(v: Option<&JsValue>) -> f64 {
    v.map(JsValue::to_number).unwrap_or(f64::NAN)
}

/// `a === b` for values read from JSON: objects and arrays are never the same object.
pub fn strict_eq(a: Option<&JsValue>, b: Option<&JsValue>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(JsValue::Null), Some(JsValue::Null)) => true,
        (Some(JsValue::Bool(x)), Some(JsValue::Bool(y))) => x == y,
        (Some(JsValue::Num(x)), Some(JsValue::Num(y))) => x == y,
        (Some(JsValue::Str(x)), Some(JsValue::Str(y))) => x == y,
        _ => false,
    }
}

/// `a ?? b`: the value unless it is null or undefined.
pub fn nullish(v: Option<&JsValue>) -> Option<&JsValue> {
    v.filter(|x| **x != JsValue::Null)
}

/// An object built in insertion order. `None` values are left out, as `JSON.stringify` drops `undefined`.
pub fn obj(pairs: Vec<(&str, Option<JsValue>)>) -> JsValue {
    JsValue::Obj(pairs.into_iter().filter_map(|(k, v)| v.map(|v| (k.to_string(), v))).collect())
}

/// `array.sort((a, b) => compare(a, b))` as V8 runs it. With a consistent comparator this is any stable sort; with
/// one that returns NaN (a missing placement makes `a.placement - b.placement` NaN) the result depends on the exact
/// steps, so short arrays follow V8's TimSort (array-sort.tq): one leading run, then binary insertion. Longer arrays, which these
/// routes never sort, fall back to a stable sort.
pub fn js_sort<T>(xs: &mut [T], compare: impl Fn(&T, &T) -> f64) {
    // V8's SortCompare reads a NaN result as 0.
    let compare = |a: &T, b: &T| {
        let v = compare(a, b);
        if v.is_nan() { 0.0 } else { v }
    };
    let n = xs.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        xs.sort_by(|a, b| compare(a, b).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
        return;
    }
    // CountAndMakeRun
    let mut run = 2;
    let descending = compare(&xs[1], &xs[0]) < 0.0;
    for i in 2..n {
        let order = compare(&xs[i], &xs[i - 1]);
        if if descending { order >= 0.0 } else { order < 0.0 } {
            break;
        }
        run += 1;
    }
    if descending {
        xs[..run].reverse();
    }
    // BinaryInsertionSort of the rest
    for start in run..n {
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if compare(&xs[start], &xs[mid]) < 0.0 { right = mid } else { left = mid + 1 }
        }
        xs[left..=start].rotate_right(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_like_v8_even_with_nan() {
        // Placements with one missing: what Node prints for [3,1,NaN,2,8,5,4,7].sort((a, b) => a - b).
        let mut xs = [3.0, 1.0, f64::NAN, 2.0, 8.0, 5.0, 4.0, 7.0];
        js_sort(&mut xs, |a, b| a - b);
        let shown: Vec<String> = xs.iter().map(|x| x.to_string()).collect();
        assert_eq!(shown.join(","), "1,2,3,NaN,4,5,7,8");
        let mut ys = [(2, 'a'), (1, 'b'), (2, 'c'), (1, 'd')];
        js_sort(&mut ys, |a, b| f64::from(a.0 - b.0));
        assert_eq!(ys, [(1, 'b'), (1, 'd'), (2, 'a'), (2, 'c')]);
    }

    #[test]
    fn writes_numbers_like_javascript() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"), (-0.0, "0"), (1.0, "1"), (-3.0, "-3"), (1.5, "1.5"), (0.1, "0.1"), (100.0, "100"),
            (1e21, "1e+21"), (1.5e21, "1.5e+21"), (123456789012345680000.0, "123456789012345680000"),
            (1e-7, "1e-7"), (1.5e-7, "1.5e-7"), (0.000001, "0.000001"), (0.0000015, "0.0000015"),
            (2f64.powi(60), "1152921504606847000"), (1.7e12, "1700000000000"), (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"), (f64::NAN, "NaN"), (f64::INFINITY, "Infinity"),
        ];
        for (x, want) in cases {
            assert_eq!(number_string(*x), *want, "{x}");
        }
    }

    #[test]
    fn round_trips_like_json_parse_then_stringify() {
        let cases: &[(&str, &str)] = &[
            (r#"{"b":1,"a":[1.0,2.50,1e2,-0,12345678901234567890],"c":null}"#, r#"{"b":1,"a":[1,2.5,100,0,12345678901234567000],"c":null}"#),
            (r#"{"a":1,"b":2,"a":3}"#, r#"{"a":3,"b":2}"#),
            ("\"tab\\t\\u0001 é \\u00e9 \\\" \\\\ \\/ \\u2028\"", "\"tab\\t\\u0001 é é \\\" \\\\ / \u{2028}\""),
            (" [ true , false ] ", "[true,false]"),
        ];
        for (input, want) in cases {
            assert_eq!(parse(input).unwrap().stringify(), *want, "{input}");
        }
        assert!(parse_body(b"{bad").is_none());
        assert_eq!(parse_body("\u{feff}{\"k\":1}".as_bytes()).unwrap().stringify(), r#"{"k":1}"#);
    }

    #[test]
    fn converts_like_string_and_number() {
        let v = |s: &str| parse(s).unwrap();
        assert_eq!(v(r#"["a",null,1.5,[2,3],{}]"#).to_js_string(), "a,,1.5,2,3,[object Object]");
        assert_eq!(js_string(None), "undefined");
        assert_eq!(v("null").to_js_string(), "null");
        assert_eq!(v("[]").to_number(), 0.0);
        assert_eq!(v(r#"["7"]"#).to_number(), 7.0);
        assert!(v("{}").to_number().is_nan());
        assert!(!v("0").truthy() && !v(r#""""#).truthy() && v("[]").truthy() && v("{}").truthy());
        assert!(strict_eq(None, None) && !strict_eq(Some(&v("{}")), Some(&v("{}"))) && strict_eq(Some(&v("1")), Some(&v("1.0"))));
    }
}
