//! Data layer for the runs visualizer. Port of `sia/web/runs.py`.
//!
//! Pure functions that read the `runs/` directory tree and turn it into
//! JSON-serializable models. No HTTP here, so it is unit-testable in isolation.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Files surfaced as first-class artifacts in the UI (label -> filename), in order.
pub const TEXT_ARTIFACTS: &[(&str, &str)] = &[
    ("target_agent", "target_agent.py"),
    ("meta_prompt", "meta_agent_prompt.txt"),
    ("feedback_prompt", "feedback_agent_prompt.txt"),
    ("improvement", "improvement.md"),
    ("eval_log", "evaluation.log"),
    ("stdout_log", "target_agent_stdout.log"),
];

fn artifact_filename(label: &str) -> Option<&'static str> {
    TEXT_ARTIFACTS
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, f)| *f)
}

/// Candidate names for the structured evaluation summary, in priority order.
const EVAL_RESULT_NAMES: &[&str] = &["evaluation_results.json", "results.json"];

// --------------------------------------------------------------------------- //
// Models
// --------------------------------------------------------------------------- //
#[derive(Debug, Clone, Serialize, Default)]
pub struct EvalSummary {
    pub total: Option<i64>,
    pub correct: Option<i64>,
    pub incorrect: Option<i64>,
    pub missing: Option<i64>,
    pub invalid: Option<i64>,
    pub accuracy_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerationSummary {
    pub name: String,
    pub index: i64,
    pub eval: Option<EvalSummary>,
    pub has_target_agent: bool,
    pub has_improvement: bool,
    pub num_trajectories: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub name: String,
    pub index: i64,
    pub task: Option<String>,
    pub meta_model: Option<String>,
    pub task_model: Option<String>,
    pub agent_impl: Option<String>,
    pub started: Option<String>,
    pub max_generations: Option<i64>,
    pub num_generations: i64,
    pub best_accuracy_percent: Option<f64>,
    pub generations: Vec<GenerationSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DomainStat {
    pub domain: String,
    pub total: i64,
    pub correct: i64,
    pub accuracy_percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerationDetail {
    pub name: String,
    pub index: i64,
    pub eval: Option<EvalSummary>,
    pub domains: Vec<DomainStat>,
    pub artifacts: Vec<String>,
    pub trajectories: Vec<i64>,
    pub has_openhands: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunDetail {
    pub name: String,
    pub index: i64,
    pub task: Option<String>,
    pub meta_model: Option<String>,
    pub task_model: Option<String>,
    pub agent_impl: Option<String>,
    pub started: Option<String>,
    pub max_generations: Option<i64>,
    pub profiles: Option<Value>,
    pub context_md: Option<String>,
    pub generations: Vec<GenerationDetail>,
}

// --------------------------------------------------------------------------- //
// Filesystem helpers
// --------------------------------------------------------------------------- //
fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn eval_results_path(gen_dir: &Path) -> Option<PathBuf> {
    for name in EVAL_RESULT_NAMES {
        let candidate = gen_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_dir_index(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("run_")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn gen_dir_index(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("gen_")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn exec_file_qid(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("execution_q")?;
    let rest = rest.strip_suffix(".json")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn as_i64(v: &Value) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        Some(i)
    } else {
        v.as_u64().map(|u| u as i64)
    }
}

fn eval_summary(gen_dir: &Path) -> Option<EvalSummary> {
    let path = eval_results_path(gen_dir)?;
    let data = read_json(&path)?;
    let obj = data.as_object()?;

    let mut pct = obj.get("accuracy_percent").and_then(|v| v.as_f64());
    if pct.is_none() {
        if let Some(acc) = obj.get("accuracy").and_then(|v| v.as_f64()) {
            pct = Some(acc * 100.0);
        }
    }

    // total_questions or total (Python `or`: first truthy)
    let total = {
        let tq = obj.get("total_questions").and_then(as_i64);
        match tq {
            Some(n) if n != 0 => Some(n),
            _ => obj.get("total").and_then(as_i64),
        }
    };

    Some(EvalSummary {
        total,
        correct: obj.get("correct").and_then(as_i64),
        incorrect: obj.get("incorrect").and_then(as_i64),
        missing: obj.get("missing").and_then(as_i64),
        invalid: obj.get("invalid").and_then(as_i64),
        accuracy_percent: pct,
    })
}

fn trajectory_ids(gen_dir: &Path) -> Vec<i64> {
    let exec_dir = gen_dir.join("agent_execution");
    if !exec_dir.is_dir() {
        return Vec::new();
    }
    let mut ids: Vec<i64> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&exec_dir) {
        for entry in rd.flatten() {
            if let Some(qid) = exec_file_qid(&entry.file_name().to_string_lossy()) {
                ids.push(qid);
            }
        }
    }
    ids.sort();
    ids
}

fn gen_dirs(run_dir: &Path) -> Vec<(i64, PathBuf)> {
    let mut found: Vec<(i64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(run_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(idx) = gen_dir_index(&entry.file_name().to_string_lossy()) {
                    found.push((idx, path));
                }
            }
        }
    }
    found.sort_by_key(|t| t.0);
    found
}

/// Extract the leading `**Key**: value` metadata block from context.md.
pub fn parse_context_md(text: &str) -> std::collections::HashMap<String, String> {
    let re = regex::Regex::new(r"^\*\*([^*]+)\*\*:\s*(.*)$").unwrap();
    let mut meta = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("## ") {
            break;
        }
        if let Some(caps) = re.captures(line) {
            meta.insert(caps[1].trim().to_lowercase(), caps[2].trim().to_string());
        }
    }
    meta
}

// --------------------------------------------------------------------------- //
// Public API
// --------------------------------------------------------------------------- //

/// Summaries for every `run_<id>` directory, newest id first.
pub fn list_runs(runs_root: &Path) -> Vec<RunSummary> {
    if !runs_root.is_dir() {
        return Vec::new();
    }
    let mut runs: Vec<RunSummary> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(runs_root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(idx) = run_dir_index(&entry.file_name().to_string_lossy()) {
                    runs.push(run_summary(&path, idx));
                }
            }
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.index));
    runs
}

fn run_summary(run_dir: &Path, index: i64) -> RunSummary {
    let context = run_dir.join("context.md");
    let meta = if context.is_file() {
        parse_context_md(&read_text(&context).unwrap_or_default())
    } else {
        std::collections::HashMap::new()
    };

    let mut gens: Vec<GenerationSummary> = Vec::new();
    let mut best: Option<f64> = None;
    for (gen_index, gen_dir) in gen_dirs(run_dir) {
        let ev = eval_summary(&gen_dir);
        if let Some(e) = &ev {
            if let Some(pct) = e.accuracy_percent {
                best = Some(best.map_or(pct, |b| b.max(pct)));
            }
        }
        gens.push(GenerationSummary {
            name: gen_dir.file_name().unwrap().to_string_lossy().into_owned(),
            index: gen_index,
            eval: ev,
            has_target_agent: gen_dir.join("target_agent.py").is_file(),
            has_improvement: gen_dir.join("improvement.md").is_file(),
            num_trajectories: trajectory_ids(&gen_dir).len() as i64,
        });
    }

    RunSummary {
        name: run_dir.file_name().unwrap().to_string_lossy().into_owned(),
        index,
        task: meta.get("task").cloned(),
        meta_model: meta.get("meta model").cloned(),
        task_model: meta.get("task model").cloned(),
        agent_impl: meta.get("agent impl").cloned(),
        started: meta.get("started").cloned(),
        max_generations: meta.get("max generations").and_then(|s| as_int(Some(s))),
        num_generations: gens.len() as i64,
        best_accuracy_percent: best,
        generations: gens,
    }
}

/// Full detail for a single run, or `None` if not found.
pub fn get_run(runs_root: &Path, run_name: &str) -> Option<RunDetail> {
    let run_dir = safe_child(runs_root, run_name)?;
    if !run_dir.is_dir() {
        return None;
    }
    let index = run_dir_index(&run_dir.file_name()?.to_string_lossy())?;

    let context = run_dir.join("context.md");
    let (context_md, meta) = if context.is_file() {
        let text = read_text(&context);
        let meta = parse_context_md(text.as_deref().unwrap_or(""));
        (text, meta)
    } else {
        (None, std::collections::HashMap::new())
    };

    let generations = gen_dirs(&run_dir)
        .into_iter()
        .map(|(gi, gd)| generation_detail(&gd, gi))
        .collect();

    let profiles = read_json(&run_dir.join("profiles.json")).filter(|v| v.is_object());

    Some(RunDetail {
        name: run_dir.file_name()?.to_string_lossy().into_owned(),
        index,
        task: meta.get("task").cloned(),
        meta_model: meta.get("meta model").cloned(),
        task_model: meta.get("task model").cloned(),
        agent_impl: meta.get("agent impl").cloned(),
        started: meta.get("started").cloned(),
        max_generations: meta.get("max generations").and_then(|s| as_int(Some(s))),
        profiles,
        context_md,
        generations,
    })
}

fn generation_detail(gen_dir: &Path, index: i64) -> GenerationDetail {
    let artifacts: Vec<String> = TEXT_ARTIFACTS
        .iter()
        .filter(|(_, fname)| gen_dir.join(fname).is_file())
        .map(|(label, _)| label.to_string())
        .collect();
    GenerationDetail {
        name: gen_dir.file_name().unwrap().to_string_lossy().into_owned(),
        index,
        eval: eval_summary(gen_dir),
        domains: domain_stats(gen_dir),
        artifacts,
        trajectories: trajectory_ids(gen_dir),
        has_openhands: gen_dir.join("openhands_trajectory").is_dir(),
    }
}

fn domain_stats(gen_dir: &Path) -> Vec<DomainStat> {
    let path = match eval_results_path(gen_dir) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let data = match read_json(&path) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let details = match data.get("details").and_then(|v| v.as_array()) {
        Some(d) => d,
        None => return Vec::new(),
    };

    // domain -> (total, correct), preserving first-seen via a Vec.
    let mut order: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    for row in details {
        let row = match row.as_object() {
            Some(r) => r,
            None => continue,
        };
        let domain = row
            .get("domain")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown")
            .to_string();
        let entry = buckets.entry(domain.clone()).or_insert_with(|| {
            order.push(domain.clone());
            (0, 0)
        });
        entry.0 += 1;
        if row
            .get("is_correct")
            .map(|v| v.as_bool().unwrap_or(false) || truthy(v))
            .unwrap_or(false)
        {
            entry.1 += 1;
        }
    }

    let mut stats: Vec<DomainStat> = buckets
        .into_iter()
        .map(|(domain, (total, correct))| DomainStat {
            domain,
            total,
            correct,
            accuracy_percent: if total != 0 {
                correct as f64 / total as f64 * 100.0
            } else {
                0.0
            },
        })
        .collect();
    stats.sort_by(|a, b| a.domain.cmp(&b.domain));
    stats
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The full per-question `details` array from evaluation results.
pub fn get_eval_details(runs_root: &Path, run_name: &str, gen_name: &str) -> Option<Vec<Value>> {
    let gen_dir = resolve_gen(runs_root, run_name, gen_name)?;
    let path = eval_results_path(&gen_dir)?;
    let data = read_json(&path)?;
    data.get("details").and_then(|v| v.as_array()).cloned()
}

/// Read one of the known text artifacts (by label, not raw path).
pub fn get_artifact_text(
    runs_root: &Path,
    run_name: &str,
    gen_name: &str,
    label: &str,
) -> Option<String> {
    let fname = artifact_filename(label)?;
    let gen_dir = resolve_gen(runs_root, run_name, gen_name)?;
    read_text(&gen_dir.join(fname))
}

/// Per-question chat log, normalized to `[{role, text}]` turns.
pub fn get_trajectory(
    runs_root: &Path,
    run_name: &str,
    gen_name: &str,
    qid: i64,
) -> Option<Vec<Value>> {
    let gen_dir = resolve_gen(runs_root, run_name, gen_name)?;
    let path = gen_dir
        .join("agent_execution")
        .join(format!("execution_q{qid}.json"));
    let data = read_json(&path)?;
    let arr = data.as_array()?;
    Some(
        arr.iter()
            .filter(|m| m.is_object())
            .map(|m| normalize_turn(m.as_object().unwrap()))
            .collect(),
    )
}

fn normalize_turn(msg: &serde_json::Map<String, Value>) -> Value {
    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let role = if msg.contains_key("role") && !msg.get("role").unwrap().is_string() {
        stringify(msg.get("role").unwrap())
    } else {
        role
    };
    let content = msg.get("content");
    let text = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(block_text)
            .collect::<Vec<_>>()
            .join("\n\n")
            .trim()
            .to_string(),
        None | Some(Value::Null) => String::new(),
        Some(other) => stringify(other),
    };
    serde_json::json!({"role": role, "text": text})
}

fn block_text(block: &Value) -> String {
    match block {
        Value::String(s) => s.clone(),
        Value::Object(b) => {
            let btype = b.get("type").and_then(|v| v.as_str());
            if btype == Some("text") || b.contains_key("text") {
                return b.get("text").map(stringify_plain).unwrap_or_default();
            }
            match btype {
                Some("tool_use") => {
                    let args = serde_json::to_string_pretty(
                        b.get("input").unwrap_or(&serde_json::json!({})),
                    )
                    .unwrap_or_default();
                    let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("[tool_use: {name}]\n{args}")
                }
                Some("tool_result") => format!(
                    "[tool_result]\n{}",
                    stringify(b.get("content").unwrap_or(&Value::Null))
                ),
                _ => stringify(block),
            }
        }
        other => stringify(other),
    }
}

/// `str(block.get("text", ""))` — text field rendered plainly.
fn stringify_plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn stringify(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(block_text).collect::<Vec<_>>().join("\n"),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// OpenHands session directory names for a generation.
pub fn list_openhands_sessions(
    runs_root: &Path,
    run_name: &str,
    gen_name: &str,
) -> Option<Vec<String>> {
    let gen_dir = resolve_gen(runs_root, run_name, gen_name)?;
    let root = gen_dir.join("openhands_trajectory");
    if !root.is_dir() {
        return Some(Vec::new());
    }
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Some(names)
}

/// OpenHands event JSON objects for a session.
pub fn get_openhands_events(
    runs_root: &Path,
    run_name: &str,
    gen_name: &str,
    session: &str,
) -> Option<Vec<Value>> {
    let gen_dir = resolve_gen(runs_root, run_name, gen_name)?;
    let session_dir = safe_child(&gen_dir.join("openhands_trajectory"), session)?;
    if !session_dir.is_dir() {
        return None;
    }
    let events_dir = session_dir.join("events");
    if !events_dir.is_dir() {
        return Some(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&events_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut events: Vec<Value> = Vec::new();
    for p in paths {
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(v) = read_json(&p) {
                if v.is_object() {
                    events.push(v);
                }
            }
        }
    }
    Some(events)
}

// --------------------------------------------------------------------------- //
// Path safety
// --------------------------------------------------------------------------- //
fn safe_child(parent: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return None;
    }
    let resolved = match (parent.join(name)).canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // Path may not exist yet; fall back to a lexical join + parent check.
            let joined = parent.join(name);
            let parent_canon = parent.canonicalize().ok()?;
            if joined.starts_with(&parent_canon) || joined.starts_with(parent) {
                return Some(joined);
            }
            return None;
        }
    };
    let parent_resolved = parent.canonicalize().ok()?;
    if resolved.starts_with(&parent_resolved) {
        Some(resolved)
    } else {
        None
    }
}

/// Resolve `runs_root/run_name/gen_name`, refusing traversal and non-matching names.
pub fn resolve_gen(runs_root: &Path, run_name: &str, gen_name: &str) -> Option<PathBuf> {
    let run_dir = safe_child(runs_root, run_name)?;
    run_dir_index(run_name)?;
    let gen_dir = safe_child(&run_dir, gen_name)?;
    if gen_dir_index(gen_name).is_none() || !gen_dir.is_dir() {
        return None;
    }
    Some(gen_dir)
}

fn as_int(value: Option<&String>) -> Option<i64> {
    value.and_then(|s| s.parse().ok())
}
