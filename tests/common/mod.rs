//! Shared test helpers — Rust port of `tests/golden_master.py`.

#![allow(dead_code)]

use std::path::PathBuf;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Replace concrete "YYYY-MM-DD HH:MM:SS" timestamps with a stable placeholder.
pub fn normalize_timestamps(text: &str) -> String {
    // Matches \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}
    let re = regex::Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}").unwrap();
    re.replace_all(text, "<TS>").into_owned()
}

/// Replace volatile absolute paths with stable placeholders (raw + canonicalized form).
pub fn normalize_paths(mut text: String, replacements: &[(&str, &str)]) -> String {
    for (raw, placeholder) in replacements {
        let variants = {
            let mut v = vec![raw.to_string()];
            if let Ok(canon) = std::fs::canonicalize(raw) {
                v.push(canon.to_string_lossy().into_owned());
            }
            v
        };
        for variant in variants {
            text = text.replace(&variant, placeholder);
        }
    }
    text
}

/// Assert `actual` matches committed golden `name` (or write it under `UPDATE_GOLDEN=1`).
pub fn assert_golden(name: &str, actual: &str) {
    let path = golden_dir().join(name);
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    assert!(
        path.exists(),
        "Missing golden '{name}'. Generate with: UPDATE_GOLDEN=1 cargo test"
    );
    let expected = std::fs::read_to_string(&path).unwrap();
    if actual != expected {
        // Produce a small line-diff to make mismatches debuggable.
        let exp_lines: Vec<&str> = expected.lines().collect();
        let act_lines: Vec<&str> = actual.lines().collect();
        let mut diff = String::new();
        let max = exp_lines.len().max(act_lines.len());
        for i in 0..max {
            let e = exp_lines.get(i).copied().unwrap_or("<none>");
            let a = act_lines.get(i).copied().unwrap_or("<none>");
            if e != a {
                diff.push_str(&format!(
                    "line {}:\n  expected: {:?}\n  actual:   {:?}\n",
                    i + 1,
                    e,
                    a
                ));
            }
        }
        panic!("Golden mismatch for '{name}':\n{diff}");
    }
}
