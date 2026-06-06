//! Python-faithful JSON serialization (`json.dumps(value, indent=2)` with the
//! default `ensure_ascii=True`).
//!
//! serde_json's pretty printer matches Python for ASCII content but emits raw
//! UTF-8 for non-ASCII, whereas Python escapes every character outside the
//! printable-ASCII range (0x20..=0x7e) as `\uXXXX` (with surrogate pairs above
//! U+FFFF). SIA embeds `json.dumps` output verbatim into prompts and the on-disk
//! `context.md` / feedback context, so non-ASCII task data (e.g. LawBench, which
//! is Chinese) would otherwise diverge byte-for-byte. This module reproduces
//! Python's exact output.

use serde_json::Value;

/// Equivalent of Python `json.dumps(value, indent=2)` (ensure_ascii=True).
pub fn dumps_indent2(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level * 2 {
        out.push(' ');
    }
}

fn write_value(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&format_number(n)),
        Value::String(s) => write_string(out, s),
        Value::Array(arr) => {
            if arr.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                write_indent(out, level + 1);
                write_value(out, item, level + 1);
            }
            out.push('\n');
            write_indent(out, level);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                write_indent(out, level + 1);
                write_string(out, k);
                out.push_str(": ");
                write_value(out, v, level + 1);
            }
            out.push('\n');
            write_indent(out, level);
            out.push('}');
        }
    }
}

/// Format a JSON number like Python's `json.dumps`.
///
/// serde_json and CPython both use shortest round-trip formatting and agree on
/// integers, ordinary decimals, and ≥2-digit exponents. They differ in one place:
/// CPython zero-pads a scientific-notation exponent to at least two digits
/// (`1e-7` → `1e-07`), serde does not. We normalize that so non-ASCII *and* numeric
/// output match Python byte-for-byte across the full range.
fn format_number(n: &serde_json::Number) -> String {
    pad_exponent(&n.to_string())
}

fn pad_exponent(s: &str) -> String {
    let Some(epos) = s.find(['e', 'E']) else {
        return s.to_string();
    };
    let mantissa = &s[..epos];
    let exp = &s[epos + 1..];
    let (sign, digits) = match exp.strip_prefix('-') {
        Some(d) => ('-', d),
        None => ('+', exp.strip_prefix('+').unwrap_or(exp)),
    };
    let digits = if digits.len() < 2 {
        format!("{digits:0>2}")
    } else {
        digits.to_string()
    };
    format!("{mantissa}e{sign}{digits}")
}

/// Encode a string exactly like Python's `c_encode_basestring_ascii`.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (' '..='~').contains(&c) => out.push(c),
            c => {
                let cp = c as u32;
                if cp > 0xFFFF {
                    // Encode as a UTF-16 surrogate pair (Python does this).
                    let v = cp - 0x1_0000;
                    let hi = 0xD800 + (v >> 10);
                    let lo = 0xDC00 + (v & 0x3FF);
                    out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
                } else {
                    out.push_str(&format!("\\u{cp:04x}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Reference outputs captured from CPython `json.dumps(value, indent=2)`.
    #[test]
    fn test_cjk_is_escaped() {
        let v = json!({"charge": "故意伤害罪", "n": 198});
        assert_eq!(
            dumps_indent2(&v),
            "{\n  \"charge\": \"\\u6545\\u610f\\u4f24\\u5bb3\\u7f6a\",\n  \"n\": 198\n}"
        );
    }

    #[test]
    fn test_emoji_surrogate_pair() {
        let v = json!({"msg": "ok ✓ 🔧 done"});
        assert_eq!(
            dumps_indent2(&v),
            "{\n  \"msg\": \"ok \\u2713 \\ud83d\\udd27 done\"\n}"
        );
    }

    #[test]
    fn test_astral_surrogate_pair() {
        let v = json!({"x": "𝔘𝔫𝔦"});
        assert_eq!(
            dumps_indent2(&v),
            "{\n  \"x\": \"\\ud835\\udd18\\ud835\\udd2b\\ud835\\udd26\"\n}"
        );
    }

    #[test]
    fn test_control_chars() {
        let v = json!({"t": "a\tb\nc\r\u{01}\u{1f}"});
        assert_eq!(
            dumps_indent2(&v),
            "{\n  \"t\": \"a\\tb\\nc\\r\\u0001\\u001f\"\n}"
        );
    }

    #[test]
    fn test_slash_and_angles_not_escaped() {
        let v = json!({"path": "a/b</c>"});
        assert_eq!(dumps_indent2(&v), "{\n  \"path\": \"a/b</c>\"\n}");
    }

    #[test]
    fn test_empty_containers() {
        assert_eq!(dumps_indent2(&json!({})), "{}");
        assert_eq!(dumps_indent2(&json!([])), "[]");
    }

    #[test]
    fn test_nested_and_floats() {
        let v = json!([{"role": "user", "content": "café ☕"}]);
        assert_eq!(
            dumps_indent2(&v),
            "[\n  {\n    \"role\": \"user\",\n    \"content\": \"caf\\u00e9 \\u2615\"\n  }\n]"
        );
        assert_eq!(
            dumps_indent2(&json!({"a": 0.9, "b": 1.0, "c": 50.0})),
            "{\n  \"a\": 0.9,\n  \"b\": 1.0,\n  \"c\": 50.0\n}"
        );
    }

    #[test]
    fn test_number_formatting_matches_python() {
        // Captured from CPython json.dumps. Integers exact; decimals shortest
        // round-trip; scientific exponents zero-padded to >= 2 digits.
        assert_eq!(
            dumps_indent2(&json!(1_000_000_000_000_000_i64)),
            "1000000000000000"
        );
        assert_eq!(
            dumps_indent2(&json!(9_007_199_254_740_993_i64)),
            "9007199254740993"
        );
        assert_eq!(dumps_indent2(&json!(0.123456789)), "0.123456789");
        assert_eq!(dumps_indent2(&json!(12345.678)), "12345.678");
        assert_eq!(dumps_indent2(&json!(1e16)), "1e+16");
        assert_eq!(dumps_indent2(&json!(1e20)), "1e+20");
        assert_eq!(dumps_indent2(&json!(1e-7)), "1e-07");
        assert_eq!(dumps_indent2(&json!(1.5e-7)), "1.5e-07");
        assert_eq!(dumps_indent2(&json!(2.5e-3)), "0.0025");
    }

    #[test]
    fn test_ascii_matches_serde_pretty() {
        // For ASCII, output must equal serde_json's pretty printer (which the golden
        // tests proved equals Python). Guard against drift.
        let v = json!({"messages": [{"role": "user", "content": "hi"}]});
        assert_eq!(dumps_indent2(&v), serde_json::to_string_pretty(&v).unwrap());
    }
}
