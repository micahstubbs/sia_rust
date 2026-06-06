//! The target agent's *reference*: where its improvable seed code + deps come from.
//! Port of `sia/agent_reference.py`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{SiaError, SiaResult};
use crate::layout::{self, names, TaskLayout};

/// A parsed `agent_reference` spec (paths already resolved to absolute).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentReference {
    /// "default" | "file" | "dir"
    pub kind: String,
    /// abs path to the file (file) or directory (dir); None for default
    pub source: Option<PathBuf>,
    /// filename within the directory (dir only)
    pub entrypoint: Option<String>,
}

impl AgentReference {
    pub fn default_ref() -> Self {
        AgentReference {
            kind: "default".to_string(),
            source: None,
            entrypoint: None,
        }
    }
}

/// An `AgentReference` resolved against a concrete task, ready to use.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAgentReference {
    /// entrypoint text to embed in the prompt (default/file); None for a dir
    pub inline_seed: Option<String>,
    /// directory whose contents are copied into each gen working dir (dir only)
    pub ref_dir: Option<PathBuf>,
    /// filename the agent should treat as the starting point
    pub entrypoint: String,
    /// requirements.txt to install + carry forward, if present
    pub requirements: Option<PathBuf>,
}

/// Resolve a path like Python's `Path.resolve()` (non-strict): absolute + normalized,
/// resolving symlinks when the path exists.
fn resolve_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| PathBuf::from(layout::abspath(&p.to_string_lossy())))
}

/// Parse a raw `agent_reference` value from a profile JSON into an `AgentReference`.
pub fn parse_agent_reference(
    spec: Option<&serde_json::Value>,
    base_dir: Option<&Path>,
) -> SiaResult<AgentReference> {
    match spec {
        None | Some(serde_json::Value::Null) => return Ok(AgentReference::default_ref()),
        Some(serde_json::Value::String(s)) if s == "default" => {
            return Ok(AgentReference::default_ref())
        }
        _ => {}
    }

    let obj = match spec {
        Some(serde_json::Value::Object(o)) if o.contains_key("source") => o,
        _ => {
            return Err(SiaError::new(
                "agent_reference must be \"default\" or an object with a \"source\" field",
            ))
        }
    };

    let base = base_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let source_str = obj
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SiaError::new("agent_reference \"source\" must be a string"))?;
    let mut source = PathBuf::from(source_str);
    if !source.is_absolute() {
        source = base.join(&source);
    }
    let source = resolve_path(&source);

    let entrypoint = obj
        .get("entrypoint")
        .and_then(|v| v.as_str())
        .map(String::from);

    if source.is_dir() {
        Ok(AgentReference {
            kind: "dir".to_string(),
            source: Some(source),
            entrypoint,
        })
    } else if source.is_file() {
        Ok(AgentReference {
            kind: "file".to_string(),
            source: Some(source),
            entrypoint: None,
        })
    } else {
        Err(SiaError::new(format!(
            "agent_reference source not found: {}",
            source.display()
        )))
    }
}

/// Resolve an `AgentReference` against a concrete task into a `ResolvedAgentReference`.
pub fn resolve_agent_reference(
    refr: &AgentReference,
    task_layout: &TaskLayout,
) -> SiaResult<ResolvedAgentReference> {
    match refr.kind.as_str() {
        "default" => {
            let ref_dir = PathBuf::from(task_layout.reference_dir());
            let seed = std::fs::read_to_string(ref_dir.join(names::REFERENCE_AGENT_FILE))
                .map_err(|e| SiaError::new(format!("Could not read reference agent: {e}")))?;
            let reqs = ref_dir.join(names::REQUIREMENTS_TXT);
            Ok(ResolvedAgentReference {
                inline_seed: Some(seed),
                ref_dir: None,
                entrypoint: names::REFERENCE_AGENT_FILE.to_string(),
                requirements: if reqs.is_file() { Some(reqs) } else { None },
            })
        }
        "file" => {
            let source = refr.source.as_ref().expect("file reference has source");
            let seed = std::fs::read_to_string(source)
                .map_err(|e| SiaError::new(format!("Could not read reference file: {e}")))?;
            let entrypoint = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(ResolvedAgentReference {
                inline_seed: Some(seed),
                ref_dir: None,
                entrypoint,
                requirements: None,
            })
        }
        _ => {
            // kind == "dir"
            let source = refr.source.as_ref().expect("dir reference has source");
            let entrypoint = refr
                .entrypoint
                .clone()
                .unwrap_or_else(|| names::REFERENCE_AGENT_FILE.to_string());
            if !source.join(&entrypoint).is_file() {
                return Err(SiaError::new(format!(
                    "agent_reference entrypoint '{entrypoint}' not found in {}",
                    source.display()
                )));
            }
            let reqs = source.join(names::REQUIREMENTS_TXT);
            Ok(ResolvedAgentReference {
                inline_seed: None,
                ref_dir: Some(source.clone()),
                entrypoint,
                requirements: if reqs.is_file() { Some(reqs) } else { None },
            })
        }
    }
}

/// Place reference helper files + requirements.txt into a generation working dir.
pub fn copy_reference_into(
    resolved: &ResolvedAgentReference,
    gen_dir: &Path,
) -> std::io::Result<()> {
    if let Some(ref_dir) = &resolved.ref_dir {
        for entry in std::fs::read_dir(ref_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let dest = gen_dir.join(&name);
            if entry.file_type()?.is_dir() {
                copy_dir_all(&entry.path(), &dest)?;
            } else {
                std::fs::copy(entry.path(), &dest)?;
            }
        }
    } else if let Some(reqs) = &resolved.requirements {
        std::fs::copy(reqs, gen_dir.join(names::REQUIREMENTS_TXT))?;
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_dir_with_reference(tmp: &Path, requirements: bool) -> PathBuf {
        let task_dir = tmp.join("task");
        let refd = task_dir.join("reference");
        std::fs::create_dir_all(&refd).unwrap();
        std::fs::write(
            refd.join("reference_target_agent.py"),
            "print('bundled reference')",
        )
        .unwrap();
        if requirements {
            std::fs::write(refd.join("requirements.txt"), "anthropic\n").unwrap();
        }
        task_dir
    }

    #[test]
    fn test_default_resolves_to_task_reference() {
        let d = tempfile::tempdir().unwrap();
        let task_dir = task_dir_with_reference(d.path(), false);
        let layout = TaskLayout::new(task_dir.to_str().unwrap(), d.path().to_str().unwrap());

        let refr = parse_agent_reference(Some(&serde_json::json!("default")), None).unwrap();
        assert_eq!(refr.kind, "default");

        let resolved = resolve_agent_reference(&refr, &layout).unwrap();
        assert_eq!(
            resolved.inline_seed.as_deref(),
            Some("print('bundled reference')")
        );
        assert_eq!(resolved.ref_dir, None);
        assert_eq!(resolved.entrypoint, "reference_target_agent.py");
        assert_eq!(resolved.requirements, None);
    }

    #[test]
    fn test_default_picks_up_reference_requirements() {
        let d = tempfile::tempdir().unwrap();
        let task_dir = task_dir_with_reference(d.path(), true);
        let layout = TaskLayout::new(task_dir.to_str().unwrap(), d.path().to_str().unwrap());
        let resolved = resolve_agent_reference(
            &parse_agent_reference(Some(&serde_json::json!("default")), None).unwrap(),
            &layout,
        )
        .unwrap();
        assert!(resolved.requirements.is_some());
        assert_eq!(
            resolved.requirements.unwrap().file_name().unwrap(),
            "requirements.txt"
        );
    }

    #[test]
    fn test_omitted_spec_is_default() {
        assert_eq!(parse_agent_reference(None, None).unwrap().kind, "default");
    }

    #[test]
    fn test_single_file_reference() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("my_agent.py"), "print('mine')").unwrap();
        let layout = TaskLayout::new(
            d.path().join("task").to_str().unwrap(),
            d.path().to_str().unwrap(),
        );

        let refr = parse_agent_reference(
            Some(&serde_json::json!({"source": "./my_agent.py"})),
            Some(d.path()),
        )
        .unwrap();
        assert_eq!(refr.kind, "file");

        let resolved = resolve_agent_reference(&refr, &layout).unwrap();
        assert_eq!(resolved.inline_seed.as_deref(), Some("print('mine')"));
        assert_eq!(resolved.ref_dir, None);
        assert_eq!(resolved.entrypoint, "my_agent.py");
    }

    #[test]
    fn test_directory_reference_reads_from_disk() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("agent_dir");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("main.py"), "import helper").unwrap();
        std::fs::write(src.join("helper.py"), "VALUE = 1").unwrap();
        std::fs::write(src.join("requirements.txt"), "numpy\n").unwrap();
        let layout = TaskLayout::new(
            d.path().join("task").to_str().unwrap(),
            d.path().to_str().unwrap(),
        );

        let refr = parse_agent_reference(
            Some(&serde_json::json!({"source": "./agent_dir/", "entrypoint": "main.py"})),
            Some(d.path()),
        )
        .unwrap();
        assert_eq!(refr.kind, "dir");

        let resolved = resolve_agent_reference(&refr, &layout).unwrap();
        assert_eq!(resolved.inline_seed, None);
        assert_eq!(resolved.ref_dir, Some(resolve_path(&src)));
        assert_eq!(resolved.entrypoint, "main.py");
        assert!(resolved.requirements.is_some());
    }

    #[test]
    fn test_copy_reference_into_directory() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("agent_dir");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("main.py"), "x").unwrap();
        std::fs::write(src.join("helper.py"), "y").unwrap();
        let layout = TaskLayout::new(
            d.path().join("task").to_str().unwrap(),
            d.path().to_str().unwrap(),
        );
        let resolved = resolve_agent_reference(
            &parse_agent_reference(
                Some(&serde_json::json!({"source": "./agent_dir/", "entrypoint": "main.py"})),
                Some(d.path()),
            )
            .unwrap(),
            &layout,
        )
        .unwrap();

        let gen_dir = d.path().join("gen_1");
        std::fs::create_dir(&gen_dir).unwrap();
        copy_reference_into(&resolved, &gen_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(gen_dir.join("main.py")).unwrap(),
            "x"
        );
        assert_eq!(
            std::fs::read_to_string(gen_dir.join("helper.py")).unwrap(),
            "y"
        );
    }

    #[test]
    fn test_copy_reference_into_default_with_requirements() {
        let d = tempfile::tempdir().unwrap();
        let task_dir = task_dir_with_reference(d.path(), true);
        let layout = TaskLayout::new(task_dir.to_str().unwrap(), d.path().to_str().unwrap());
        let resolved = resolve_agent_reference(
            &parse_agent_reference(Some(&serde_json::json!("default")), None).unwrap(),
            &layout,
        )
        .unwrap();

        let gen_dir = d.path().join("gen_1");
        std::fs::create_dir(&gen_dir).unwrap();
        copy_reference_into(&resolved, &gen_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(gen_dir.join("requirements.txt")).unwrap(),
            "anthropic\n"
        );
        assert!(!gen_dir.join("reference_target_agent.py").exists());
    }
}
