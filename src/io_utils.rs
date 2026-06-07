//! Shared, size-limited filesystem helpers. Port of `sia/io_utils.py`.

use std::path::Path;

use crate::config::Config;

/// Default size cap, taken from the default `Config`.
pub fn default_max_bytes() -> u64 {
    Config::default().max_context_file_size
}

/// Format an integer with comma thousands separators, matching Python's `{n:,}`.
pub(crate) fn comma(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Return `(within_limit, size_bytes)` for `path`.
///
/// A file whose size equals `max_bytes` is within the limit (matches the prior `<=` check).
/// Returns `Err` if the size cannot be read (missing file, etc.).
pub fn file_size_ok<P: AsRef<Path>>(path: P, max_bytes: u64) -> std::io::Result<(bool, u64)> {
    let size = std::fs::metadata(path.as_ref())?.len();
    Ok((size <= max_bytes, size))
}

/// Read a file as UTF-8 text, returning `None` if it exceeds `max_bytes` or can't be read.
pub fn safe_read_file<P: AsRef<Path>>(path: P, max_bytes: u64) -> Option<String> {
    let path = path.as_ref();
    let (within_limit, size) = match file_size_ok(path, max_bytes) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Could not read file: {}: {e}", path.display());
            return None;
        }
    };
    if !within_limit {
        log::warn!(
            "File too large ({} bytes > {}): {}",
            comma(size),
            comma(max_bytes),
            path.display()
        );
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) => {
            log::warn!("Could not read file: {}: {e}", path.display());
            None
        }
    }
}

/// Load JSON from a file, returning `None` if it exceeds `max_bytes` or can't be parsed.
pub fn safe_load_json<P: AsRef<Path>>(path: P, max_bytes: u64) -> Option<serde_json::Value> {
    let path = path.as_ref();
    let (within_limit, size) = match file_size_ok(path, max_bytes) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Could not load JSON: {}: {e}", path.display());
            return None;
        }
    };
    if !within_limit {
        log::warn!(
            "JSON file too large ({} bytes > {}): {}",
            comma(size),
            comma(max_bytes),
            path.display()
        );
        return None;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("Could not load JSON: {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("Could not load JSON: {}: {e}", path.display());
            None
        }
    }
}

/// Write UTF-8 text to `path`.
pub fn write_text<P: AsRef<Path>>(path: P, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn test_comma_grouping_matches_python() {
        assert_eq!(comma(0), "0");
        assert_eq!(comma(999), "999");
        assert_eq!(comma(1000), "1,000");
        assert_eq!(comma(12345), "12,345");
        assert_eq!(comma(1234567), "1,234,567");
    }

    #[test]
    fn test_read_file_under_limit() {
        let d = tmp();
        let f = d.path().join("small.txt");
        std::fs::write(&f, "hello world").unwrap();
        assert_eq!(safe_read_file(&f, 1024).as_deref(), Some("hello world"));
    }

    #[test]
    fn test_read_file_over_limit() {
        let d = tmp();
        let f = d.path().join("big.txt");
        std::fs::write(&f, "x".repeat(2000)).unwrap();
        assert_eq!(safe_read_file(&f, 1000), None);
    }

    #[test]
    fn test_read_file_at_exact_limit() {
        let d = tmp();
        let f = d.path().join("exact.txt");
        let content = "a".repeat(1000);
        std::fs::write(&f, &content).unwrap();
        assert_eq!(safe_read_file(&f, 1000).as_deref(), Some(content.as_str()));
    }

    #[test]
    fn test_load_json_under_limit() {
        let d = tmp();
        let f = d.path().join("data.json");
        std::fs::write(&f, r#"{"accuracy": 0.95}"#).unwrap();
        let v = safe_load_json(&f, 4096).unwrap();
        assert_eq!(v["accuracy"], 0.95);
    }

    #[test]
    fn test_load_json_over_limit() {
        let d = tmp();
        let f = d.path().join("big.json");
        let mut fh = std::fs::File::create(&f).unwrap();
        write!(fh, "{}", serde_json::json!({"key": "x".repeat(5000)})).unwrap();
        assert_eq!(safe_load_json(&f, 1000), None);
    }

    #[test]
    fn test_load_json_nonexistent() {
        let d = tmp();
        assert_eq!(safe_load_json(d.path().join("nope.json"), 4096), None);
    }

    #[test]
    fn test_load_json_invalid_json() {
        let d = tmp();
        let f = d.path().join("bad.json");
        std::fs::write(&f, "{not valid json").unwrap();
        assert_eq!(safe_load_json(&f, 4096), None);
    }

    #[test]
    fn test_read_file_nonexistent() {
        let d = tmp();
        assert_eq!(safe_read_file(d.path().join("missing.txt"), 4096), None);
    }
}
