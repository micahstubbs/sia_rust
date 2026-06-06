//! Minimal, std-only `.env` loader so `sia run --features llm` can pick up
//! provider credentials from a file instead of requiring every key to be
//! exported in the shell. There is no `dotenv`-style dependency: this is a
//! tiny parser plus a loader that fills *gaps* in the environment.
//!
//! ## Precedence (important)
//!
//! The real process environment **always wins**. [`load_dotenv`] only sets a
//! key when it is *not already present* in [`std::env`], so exporting a
//! variable in your shell (or in CI) overrides whatever a `.env` file says.
//! This makes loading purely additive and safe to call unconditionally.
//!
//! ## Safety
//!
//! Loading is a no-op (returns `0`) when no `.env` exists or it cannot be read,
//! and it never panics. Malformed lines are skipped rather than erroring.
//!
//! ## `.gitignore`
//!
//! `.env` holds secrets and must never be committed; the repo's `.gitignore`
//! already excludes it (the `Environments` section). The placeholder
//! `.env.example` (no real keys) *is* tracked so contributors know which
//! variables to set — see [`docs/CREDENTIALS.md`](../docs/CREDENTIALS.md).

use std::path::Path;

/// Name of the dotenv file looked up in the current working directory.
const DOTENV_FILENAME: &str = ".env";

/// Load `.env` from the current working directory into the process environment,
/// setting each `KEY=VALUE` **only if `KEY` is not already set** (real env wins).
///
/// Returns the number of variables actually set. Returns `0` (a no-op) when the
/// file is absent or unreadable. Never panics.
#[must_use]
pub fn load_dotenv() -> usize {
    load_dotenv_from(Path::new(DOTENV_FILENAME))
}

/// Like [`load_dotenv`] but for an explicit path (used by tests).
fn load_dotenv_from(path: &Path) -> usize {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        // Absent or unreadable file -> no-op. Never propagate the error.
        Err(_) => return 0,
    };

    let mut loaded = 0usize;
    for (key, value) in parse_dotenv(&contents) {
        // Real environment always wins: only fill gaps.
        if std::env::var_os(&key).is_none() {
            std::env::set_var(&key, &value);
            loaded += 1;
        }
    }
    loaded
}

/// Pure parser for `.env` contents — returns `(key, value)` pairs in file order.
///
/// Supported syntax:
/// - Blank lines and `#` comment lines are skipped.
/// - `KEY=VALUE` and `export KEY=VALUE` (the `export ` prefix is stripped).
/// - Surrounding whitespace around the key and value is trimmed.
/// - Single- or double-quoted values have their *matching* surrounding quotes
///   stripped. No escape-sequence expansion is performed even for double quotes
///   (e.g. `"a\nb"` stays the literal characters `a\nb`); keep values simple.
/// - An `=` inside the value is preserved (only the first `=` splits the line).
/// - Lines without an `=` are malformed and ignored.
///
/// This is intentionally pure (no env access) so it is unit-testable.
#[must_use]
pub fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Optional `export ` prefix (POSIX shell habit).
        let line = line.strip_prefix("export ").map_or(line, str::trim);

        // Split on the first `=`; lines without one are malformed -> skip.
        let Some((key_part, value_part)) = line.split_once('=') else {
            continue;
        };

        let key = key_part.trim();
        if key.is_empty() {
            continue;
        }

        let value = strip_matching_quotes(value_part.trim());
        pairs.push((key.to_string(), value.to_string()));
    }

    pairs
}

/// Strip a single pair of matching surrounding quotes (`'...'` or `"..."`).
/// Leaves the input untouched if it is not quoted on both ends.
fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes env mutation across tests in this module so the precedence
    /// test never races another test reading/writing the same keys.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let input = "\n# a comment\n   \n# another\nKEY=value\n";
        assert_eq!(parse_dotenv(input), vec![("KEY".into(), "value".into())]);
    }

    #[test]
    fn parse_handles_export_prefix() {
        let input = "export FOO=bar";
        assert_eq!(parse_dotenv(input), vec![("FOO".into(), "bar".into())]);
    }

    #[test]
    fn parse_strips_double_and_single_quotes() {
        let input = "A=\"double\"\nB='single'";
        assert_eq!(
            parse_dotenv(input),
            vec![("A".into(), "double".into()), ("B".into(), "single".into())]
        );
    }

    #[test]
    fn parse_keeps_unmatched_or_inner_quotes() {
        // Mismatched quote chars are NOT stripped.
        let input = "A=\"oops'\nB=val\"ue";
        assert_eq!(
            parse_dotenv(input),
            vec![
                ("A".into(), "\"oops'".into()),
                ("B".into(), "val\"ue".into())
            ]
        );
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let input = "   KEY   =   value   ";
        assert_eq!(parse_dotenv(input), vec![("KEY".into(), "value".into())]);
    }

    #[test]
    fn parse_ignores_malformed_lines_without_equals() {
        let input = "NOT_AN_ASSIGNMENT\nGOOD=1\nalso bad";
        assert_eq!(parse_dotenv(input), vec![("GOOD".into(), "1".into())]);
    }

    #[test]
    fn parse_preserves_equals_inside_value() {
        let input = "URL=https://host/path?a=1&b=2";
        assert_eq!(
            parse_dotenv(input),
            vec![("URL".into(), "https://host/path?a=1&b=2".into())]
        );
    }

    #[test]
    fn parse_does_not_expand_escapes_in_double_quotes() {
        // Documented behavior: no escape expansion; backslash-n stays literal.
        let input = "A=\"a\\nb\"";
        assert_eq!(parse_dotenv(input), vec![("A".into(), "a\\nb".into())]);
    }

    #[test]
    fn parse_skips_keyless_lines() {
        let input = "=novalue\n  =x";
        assert_eq!(parse_dotenv(input), Vec::<(String, String)>::new());
    }

    #[test]
    fn load_dotenv_respects_existing_env_and_fills_gaps() {
        let _guard = ENV_LOCK.lock().unwrap();

        // Unique key names so we never collide with the host env or other tests.
        let preset_key = "SIA_ENVFILE_TEST_PRESET";
        let unset_key = "SIA_ENVFILE_TEST_UNSET";

        // Snapshot for restoration.
        let saved_preset = std::env::var_os(preset_key);
        let saved_unset = std::env::var_os(unset_key);

        // Pre-set one key in the real env; ensure the other is unset.
        std::env::set_var(preset_key, "from-real-env");
        std::env::remove_var(unset_key);

        // Run from a temp cwd so we never read the repo's real `.env`.
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".env");
        std::fs::write(
            &env_path,
            format!("{preset_key}=from-dotenv\n{unset_key}=from-dotenv\n"),
        )
        .unwrap();

        let loaded = load_dotenv_from(&env_path);

        // Only the previously-unset key was filled; the preset one is untouched.
        assert_eq!(loaded, 1, "exactly one gap should be filled");
        assert_eq!(
            std::env::var(preset_key).unwrap(),
            "from-real-env",
            "real env must win over .env"
        );
        assert_eq!(
            std::env::var(unset_key).unwrap(),
            "from-dotenv",
            "unset key should be filled from .env"
        );

        // Restore prior env state.
        match saved_preset {
            Some(v) => std::env::set_var(preset_key, v),
            None => std::env::remove_var(preset_key),
        }
        match saved_unset {
            Some(v) => std::env::set_var(unset_key, v),
            None => std::env::remove_var(unset_key),
        }
    }

    #[test]
    fn load_dotenv_is_noop_when_file_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join(".env");
        assert_eq!(load_dotenv_from(&missing), 0);
    }
}
