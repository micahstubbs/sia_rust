//! Name-or-path resolution for JSON config files (providers and profiles).
//!
//! Port of `sia/config_files.py`. A config value is either a filesystem **path**
//! (contains a path separator or ends in `.json`) or a bare **name** resolved, in
//! order, against the user directory (`$SIA_<KIND>S_DIR` else `./<kind>s`) and then
//! the bundled defaults compiled into the binary.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::assets;
use crate::error::{SiaError, SiaResult};

/// True when `value` should be treated as a filesystem path, not a bare name.
fn looks_like_path(value: &str) -> bool {
    value.ends_with(".json") || value.contains('/') || value.contains(std::path::MAIN_SEPARATOR)
}

/// User config directory: `$<env_var>` if set, else `./<default_subdir>`.
pub fn user_dir(env_var: &str, default_subdir: &str) -> PathBuf {
    match std::env::var(env_var) {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(default_subdir),
    }
}

/// Resolve `name_or_path` to `(file_text, source_label)` for a config of `kind`.
///
/// Returns `Err` (modeling `SystemExit`) with the list of available names when a
/// bare name can't be found.
pub fn read_config_text(
    name_or_path: &str,
    env_var: &str,
    subdir: &str,
    kind: &str,
) -> SiaResult<(String, String)> {
    if looks_like_path(name_or_path) {
        let path = Path::new(name_or_path);
        if !path.is_file() {
            return Err(SiaError::new(format!(
                "{kind} file not found: {name_or_path}"
            )));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| SiaError::new(format!("{kind} file not found: {name_or_path}: {e}")))?;
        return Ok((text, path.display().to_string()));
    }

    let filename = format!("{name_or_path}.json");

    let candidate = user_dir(env_var, subdir).join(&filename);
    if candidate.is_file() {
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Ok((text, candidate.display().to_string()));
        }
    }

    if let Some(text) = assets::bundled_default_text(subdir, &filename) {
        return Ok((
            text.to_string(),
            format!("<bundled>/defaults/{subdir}/{filename}"),
        ));
    }

    let names = available_names(env_var, subdir);
    let available = if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    };
    Err(SiaError::new(format!(
        "Unknown {kind} '{name_or_path}'. Available: {available} (or pass a path to a .json file)."
    )))
}

/// Sorted union of config names from the bundled defaults and the user directory.
pub fn available_names(env_var: &str, subdir: &str) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();

    for fname in assets::bundled_default_filenames(subdir) {
        names.insert(fname.trim_end_matches(".json").to_string());
    }

    let udir = user_dir(env_var, subdir);
    if udir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&udir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".json") {
                    names.insert(name.trim_end_matches(".json").to_string());
                }
            }
        }
    }

    names.into_iter().collect()
}
