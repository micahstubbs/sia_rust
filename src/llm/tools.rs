//! Sandboxed tool executors for the native Claude runner (issue #39).
//!
//! These executors mirror the Claude Agent SDK's allowed tools — `Bash`,
//! `Read`, `Write`, `Edit`, `Glob` — but run **locally**, sandboxed to a
//! `working_dir`. Every path argument is resolved under `working_dir` and any
//! attempt to escape the sandbox via `..` (or an absolute path) is rejected
//! with an error string rather than touching the filesystem outside the box.
//!
//! Each executor returns a plain `String` (the tool-result text) so the agent
//! loop can wrap it in a `tool_result` content block verbatim. Errors are
//! returned as ordinary result strings prefixed with `Error:` — the loop marks
//! the `tool_result` as `is_error` based on a leading `Error:` marker — so the
//! model can read the failure and adapt, matching the SDK's tool semantics.
//!
//! This module is intentionally framework-free and is shared: issues #40 and
//! #41 reuse the same executors and [`tool_defs`]. The whole module is gated
//! behind the non-default `llm` cargo feature.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use super::anthropic_api::ToolDef;

/// Prefix that marks a tool result as an error (drives `is_error` in the loop).
pub const ERROR_PREFIX: &str = "Error:";

/// Returns `true` if a tool-result string represents an error.
pub fn is_error_result(result: &str) -> bool {
    result.starts_with(ERROR_PREFIX)
}

/// Resolve `rel` under `working_dir`, rejecting any path that escapes the
/// sandbox via `..` components or an absolute path.
///
/// Returns the joined absolute-ish path on success, or an `Error:` string on a
/// sandbox-escape attempt. The check is purely lexical (it does not touch the
/// filesystem), so it is safe to call on paths that don't exist yet (e.g. for
/// `write_file`).
fn resolve_in_sandbox(working_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(format!(
            "{ERROR_PREFIX} path '{rel}' is absolute; paths must be relative to the working directory"
        ));
    }

    let mut depth: i32 = 0;
    for component in rel_path.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "{ERROR_PREFIX} path '{rel}' escapes the working directory sandbox"
                    ));
                }
            }
            Component::Normal(_) | Component::CurDir => {
                depth += matches!(component, Component::Normal(_)) as i32
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "{ERROR_PREFIX} path '{rel}' escapes the working directory sandbox"
                ));
            }
        }
    }

    Ok(working_dir.join(rel_path))
}

/// Execute a shell command via `sh -c` in `working_dir`, capturing combined
/// stdout+stderr and enforcing a wall-clock `timeout_secs`.
///
/// On timeout the child is killed and a timeout message is returned. A non-zero
/// exit status is reported alongside the captured output.
pub fn bash(working_dir: &Path, command: &str, timeout_secs: u64) -> String {
    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => return format!("{ERROR_PREFIX} failed to spawn shell: {e}"),
    };

    // Poll for completion with a deadline; kill on timeout.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return format!(
                        "{ERROR_PREFIX} command timed out after {timeout_secs}s and was killed: {command}"
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                return format!("{ERROR_PREFIX} failed while waiting for command: {e}");
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return format!("{ERROR_PREFIX} failed to collect command output: {e}"),
    };

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }

    let code = output.status.code();
    match code {
        Some(0) => {
            if combined.is_empty() {
                "(no output)".to_string()
            } else {
                combined
            }
        }
        Some(c) => format!("{combined}\n[exit code: {c}]"),
        None => format!("{combined}\n[terminated by signal]"),
    }
}

/// Read a file resolved under `working_dir`.
pub fn read_file(working_dir: &Path, path: &str) -> String {
    let resolved = match resolve_in_sandbox(working_dir, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match std::fs::read_to_string(&resolved) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            format!("{ERROR_PREFIX} file not found: {path}")
        }
        Err(e) => format!("{ERROR_PREFIX} failed to read '{path}': {e}"),
    }
}

/// Create or overwrite a file resolved under `working_dir`, creating parent
/// directories as needed.
pub fn write_file(working_dir: &Path, path: &str, content: &str) -> String {
    let resolved = match resolve_in_sandbox(working_dir, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = resolved.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("{ERROR_PREFIX} failed to create parent directories for '{path}': {e}");
        }
    }
    match std::fs::write(&resolved, content) {
        Ok(()) => format!("Wrote {} bytes to {path}", content.len()),
        Err(e) => format!("{ERROR_PREFIX} failed to write '{path}': {e}"),
    }
}

/// Exact string replacement in a file resolved under `working_dir`.
///
/// Errors if `old_string` is absent or occurs more than once (mirroring the
/// SDK's `Edit` tool, which requires a unique match).
pub fn edit_file(working_dir: &Path, path: &str, old_string: &str, new_string: &str) -> String {
    let resolved = match resolve_in_sandbox(working_dir, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let contents = match std::fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return format!("{ERROR_PREFIX} file not found: {path}")
        }
        Err(e) => return format!("{ERROR_PREFIX} failed to read '{path}': {e}"),
    };

    let occurrences = contents.matches(old_string).count();
    match occurrences {
        0 => format!("{ERROR_PREFIX} old_string not found in '{path}'"),
        1 => {
            let updated = contents.replacen(old_string, new_string, 1);
            match std::fs::write(&resolved, updated) {
                Ok(()) => format!("Edited {path}"),
                Err(e) => format!("{ERROR_PREFIX} failed to write '{path}': {e}"),
            }
        }
        n => format!(
            "{ERROR_PREFIX} old_string is not unique in '{path}' ({n} occurrences); provide more context"
        ),
    }
}

/// List paths matching `pattern` relative to `working_dir`, newline-joined and
/// sorted. Returned paths are relative to `working_dir`.
pub fn glob(working_dir: &Path, pattern: &str) -> String {
    // Build an absolute glob rooted at the working dir. `glob::glob` walks the
    // filesystem; we then re-relativize each hit back to the sandbox root.
    let root = working_dir.to_string_lossy();
    let full_pattern = format!("{}/{}", root.trim_end_matches('/'), pattern);

    let paths = match glob::glob(&full_pattern) {
        Ok(p) => p,
        Err(e) => return format!("{ERROR_PREFIX} invalid glob pattern '{pattern}': {e}"),
    };

    let mut matches: Vec<String> = Vec::new();
    for entry in paths.flatten() {
        let rel = entry
            .strip_prefix(working_dir)
            .unwrap_or(&entry)
            .to_string_lossy()
            .into_owned();
        matches.push(rel);
    }
    matches.sort();

    if matches.is_empty() {
        format!("(no matches for '{pattern}')")
    } else {
        matches.join("\n")
    }
}

/// The five Anthropic tool definitions exposed to the model, with names
/// matching the Claude SDK allowed tools (`Bash`, `Read`, `Write`, `Edit`,
/// `Glob`).
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "Bash".to_string(),
            description:
                "Run a shell command via `sh -c` in the working directory and return its combined \
                 stdout and stderr. Use for building, running tests, and inspecting the workspace."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The shell command to execute."}
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "Read".to_string(),
            description: "Read the contents of a file (path relative to the working directory)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the working directory."}
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "Write".to_string(),
            description:
                "Create or overwrite a file (path relative to the working directory), creating parent \
                 directories as needed."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the working directory."},
                    "content": {"type": "string", "description": "The full file content to write."}
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: "Edit".to_string(),
            description:
                "Replace an exact, unique occurrence of old_string with new_string in a file. Fails \
                 if old_string is absent or appears more than once."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the working directory."},
                    "old_string": {"type": "string", "description": "The exact text to replace (must be unique)."},
                    "new_string": {"type": "string", "description": "The replacement text."}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDef {
            name: "Glob".to_string(),
            description:
                "List files matching a glob pattern (e.g. `**/*.rs`) relative to the working directory."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "A glob pattern relative to the working directory."}
                },
                "required": ["pattern"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tmp();
        let wd = dir.path();
        let msg = write_file(wd, "notes/a.txt", "hello world");
        assert!(!is_error_result(&msg), "{msg}");
        assert!(msg.contains("11 bytes"));
        // Parent dir was created.
        assert!(wd.join("notes").is_dir());
        let read = read_file(wd, "notes/a.txt");
        assert_eq!(read, "hello world");
    }

    #[test]
    fn read_missing_file_reports_not_found() {
        let dir = tmp();
        let result = read_file(dir.path(), "nope.txt");
        assert!(is_error_result(&result), "{result}");
        assert!(result.contains("file not found"));
    }

    #[test]
    fn edit_replaces_unique_occurrence() {
        let dir = tmp();
        let wd = dir.path();
        fs::write(wd.join("f.txt"), "the quick brown fox").unwrap();
        let msg = edit_file(wd, "f.txt", "quick", "slow");
        assert!(!is_error_result(&msg), "{msg}");
        assert_eq!(
            fs::read_to_string(wd.join("f.txt")).unwrap(),
            "the slow brown fox"
        );
    }

    #[test]
    fn edit_errors_when_old_string_absent() {
        let dir = tmp();
        let wd = dir.path();
        fs::write(wd.join("f.txt"), "abc").unwrap();
        let msg = edit_file(wd, "f.txt", "xyz", "qqq");
        assert!(is_error_result(&msg), "{msg}");
        assert!(msg.contains("not found"));
    }

    #[test]
    fn edit_errors_when_old_string_not_unique() {
        let dir = tmp();
        let wd = dir.path();
        fs::write(wd.join("f.txt"), "aa aa aa").unwrap();
        let msg = edit_file(wd, "f.txt", "aa", "bb");
        assert!(is_error_result(&msg), "{msg}");
        assert!(msg.contains("not unique"));
    }

    #[test]
    fn edit_missing_file_reports_not_found() {
        let dir = tmp();
        let msg = edit_file(dir.path(), "nope.txt", "a", "b");
        assert!(is_error_result(&msg), "{msg}");
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn glob_lists_sorted_relative_paths() {
        let dir = tmp();
        let wd = dir.path();
        fs::write(wd.join("b.rs"), "").unwrap();
        fs::write(wd.join("a.rs"), "").unwrap();
        fs::write(wd.join("c.txt"), "").unwrap();
        let result = glob(wd, "*.rs");
        assert_eq!(result, "a.rs\nb.rs");
    }

    #[test]
    fn glob_no_matches_reports_message() {
        let dir = tmp();
        let result = glob(dir.path(), "*.nonexistent");
        assert!(result.contains("no matches"));
    }

    #[test]
    fn sandbox_rejects_parent_escape() {
        let dir = tmp();
        let wd = dir.path();
        let r = read_file(wd, "../secret.txt");
        assert!(is_error_result(&r), "{r}");
        assert!(r.contains("escapes the working directory sandbox"));

        let w = write_file(wd, "../evil.txt", "x");
        assert!(is_error_result(&w), "{w}");
        // The escaping write must not have created a file outside the sandbox.
        assert!(!wd.join("../evil.txt").exists());
    }

    #[test]
    fn sandbox_rejects_absolute_path() {
        let dir = tmp();
        let r = write_file(dir.path(), "/etc/passwd", "x");
        assert!(is_error_result(&r), "{r}");
        assert!(r.contains("absolute"));
    }

    #[test]
    fn sandbox_allows_internal_dotdot_that_stays_inside() {
        let dir = tmp();
        let wd = dir.path();
        fs::create_dir_all(wd.join("sub")).unwrap();
        write_file(wd, "top.txt", "hi");
        // sub/../top.txt nets to depth 0, staying inside the sandbox.
        let r = read_file(wd, "sub/../top.txt");
        assert_eq!(r, "hi");
    }

    #[test]
    fn bash_runs_and_captures_output() {
        let dir = tmp();
        let result = bash(dir.path(), "echo hello", 10);
        assert_eq!(result.trim(), "hello");
    }

    #[test]
    fn bash_reports_nonzero_exit_code() {
        let dir = tmp();
        let result = bash(dir.path(), "echo oops; exit 3", 10);
        assert!(result.contains("oops"));
        assert!(result.contains("exit code: 3"));
    }

    #[test]
    fn bash_runs_in_working_dir() {
        let dir = tmp();
        let wd = dir.path();
        fs::write(wd.join("marker.txt"), "").unwrap();
        let result = bash(wd, "ls", 10);
        assert!(result.contains("marker.txt"));
    }

    #[test]
    fn bash_times_out_and_kills_child() {
        let dir = tmp();
        let result = bash(dir.path(), "sleep 5", 1);
        assert!(is_error_result(&result), "{result}");
        assert!(result.contains("timed out"));
    }

    #[test]
    fn tool_defs_lists_five_sdk_named_tools() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Bash", "Read", "Write", "Edit", "Glob"]);
        // Each schema is an object with a properties map.
        for d in &defs {
            assert_eq!(d.input_schema["type"], "object");
            assert!(d.input_schema.get("properties").is_some());
        }
    }
}
