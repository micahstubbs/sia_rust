//! Small helpers reproducing Python string-formatting spellings used in the
//! context.md output (thousands separators, line counting, percent parsing).

/// `f"{n:,}"` — group digits with commas. `n` is non-negative.
pub fn commas_u64(n: u64) -> String {
    group(&n.to_string())
}

/// `f"{n:,}"` for a possibly-negative integer (sign then grouped magnitude).
pub fn commas_i64(n: i64) -> String {
    if n < 0 {
        format!("-{}", group(&n.unsigned_abs().to_string()))
    } else {
        group(&n.to_string())
    }
}

/// `f"{n:+,}"` — always-signed (including `+0`), grouped magnitude.
pub fn commas_i64_signed(n: i64) -> String {
    if n < 0 {
        format!("-{}", group(&n.unsigned_abs().to_string()))
    } else {
        format!("+{}", group(&n.to_string()))
    }
}

fn group(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Number of lines as Python's `len(f.readlines())` counts them: lines are
/// `\n`-terminated; a trailing partial line (no final newline) still counts; an
/// empty file is 0 lines.
pub fn count_readlines(content: &str) -> usize {
    if content.is_empty() {
        return 0;
    }
    let newlines = content.matches('\n').count();
    if content.ends_with('\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// `float(str.rstrip('%'))` — parse a number that may carry a trailing `%`.
pub fn parse_percentish(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commas() {
        assert_eq!(commas_u64(21), "21");
        assert_eq!(commas_u64(10_000_000), "10,000,000");
        assert_eq!(commas_i64_signed(48), "+48");
        assert_eq!(commas_i64_signed(0), "+0");
        assert_eq!(commas_i64_signed(-48), "-48");
    }

    #[test]
    fn test_count_readlines() {
        assert_eq!(count_readlines("print('gen 1 agent')\n"), 1);
        assert_eq!(
            count_readlines("import sys\n\n\ndef main():\n    print('x')\n\n\nmain()\n"),
            8
        );
        assert_eq!(count_readlines(""), 0);
        assert_eq!(count_readlines("abc"), 1);
        assert_eq!(count_readlines("a\nb"), 2);
    }
}
