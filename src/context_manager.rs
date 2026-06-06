//! Context Manager — tracks the evolution of agent generations in `context.md`.
//! Port of `sia/context_manager.py`.

use std::path::Path;

use chrono::Local;
use serde_json::{Map, Value};

use crate::agent_impls::run_agent;
use crate::config::Config;
use crate::io_utils::{default_max_bytes, safe_load_json, safe_read_file};
use crate::pyfmt::{commas_i64_signed, commas_u64, count_readlines, parse_percentish};

/// Input to [`ContextManager::add_generation`] (the Python `gen_data` dict).
#[derive(Debug, Clone)]
pub struct GenData {
    pub success: bool,
    pub timestamp: String,
    pub duration: f64,
    pub agent_path: String,
    pub gen_dir: String,
    pub improvement_path: Option<String>,
    pub execution_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AgentStats {
    size: u64,
    lines: usize,
}

#[derive(Debug, Clone)]
struct GenerationRecord {
    gen_num: i64,
    agent_stats: AgentStats,
    metrics: Map<String, Value>,
    success: bool,
}

/// Manages context.md for tracking generation evolution in a run.
pub struct ContextManager {
    run_dir: String,
    context_path: String,
    config: Map<String, Value>,
    cfg: Config,
    generations: Vec<GenerationRecord>,
    meta_model: String,
    agent_impl: String,
}

impl ContextManager {
    pub fn new(
        run_directory: &str,
        run_config: Map<String, Value>,
        config: Option<Config>,
    ) -> Self {
        let cfg = config.unwrap_or_default();
        let meta_model = run_config
            .get("meta_model")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| cfg.default_claude_meta_model.clone());
        let agent_impl = run_config
            .get("agent_impl")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| cfg.default_agent_impl.clone());
        let context_path = format!("{run_directory}/context.md");
        ContextManager {
            run_dir: run_directory.to_string(),
            context_path,
            config: run_config,
            cfg,
            generations: Vec::new(),
            meta_model,
            agent_impl,
        }
    }

    /// Test/inspection accessor for the resolved config.
    pub fn cfg(&self) -> &Config {
        &self.cfg
    }

    fn render_cfg(&self, key: &str, default: &str) -> String {
        match self.config.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
            Some(Value::Null) => "None".to_string(),
            Some(other) => other.to_string(),
            None => default.to_string(),
        }
    }

    /// Create context.md with header information.
    pub fn initialize(&self) {
        let basename = Path::new(&self.run_dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.run_dir.clone());
        let started = Local::now().format("%Y-%m-%d %H:%M:%S");
        let header = format!(
            "# Run Context: {basename}\n\n\
**Task**: {task}\n\
**Meta Model**: {meta}\n\
**Task Model**: {task_model}\n\
**Agent impl**: {agent_impl}\n\
**Started**: {started}\n\
**Max Generations**: {max_gen}\n\n\
---\n\n",
            basename = basename,
            task = self.render_cfg("task_dir", "N/A"),
            meta = self.render_cfg("meta_model", "N/A"),
            task_model = self.render_cfg("task_model", "N/A"),
            agent_impl = self.render_cfg("agent_impl", "N/A"),
            started = started,
            max_gen = self.render_cfg("max_gen", "N/A"),
        );
        let _ = std::fs::write(&self.context_path, header);
    }

    /// Append a generation entry to context.md.
    pub fn add_generation(&mut self, gen_num: i64, gen_data: &GenData) {
        let agent_stats = self.get_agent_stats(&gen_data.agent_path);

        // Deltas vs previous generation.
        let deltas = self.generations.last().map(|prev| {
            let prev_stats = prev.agent_stats;
            let size_pct =
                (agent_stats.size as f64 - prev_stats.size as f64) / prev_stats.size as f64 * 100.0;
            let lines_delta = agent_stats.lines as i64 - prev_stats.lines as i64;
            (size_pct, lines_delta)
        });

        let metrics = self.extract_metrics(&gen_data.gen_dir);

        let insights: Vec<String> = match &gen_data.improvement_path {
            Some(p) if Path::new(p).exists() => self.extract_insights(p),
            _ => Vec::new(),
        };

        let llm_summary = self.generate_llm_summary(gen_num);

        let entry = self.format_generation_entry(
            gen_num,
            gen_data,
            &agent_stats,
            deltas,
            &metrics,
            &insights,
            llm_summary.as_deref(),
        );

        if let Ok(mut existing) = std::fs::read_to_string(&self.context_path) {
            existing.push_str(&entry);
            existing.push_str("\n---\n\n");
            let _ = std::fs::write(&self.context_path, existing);
        } else {
            let _ = std::fs::write(&self.context_path, format!("{entry}\n---\n\n"));
        }

        self.generations.push(GenerationRecord {
            gen_num,
            agent_stats,
            metrics,
            success: gen_data.success,
        });
    }

    /// Add summary statistics at the end of context.md.
    pub fn finalize(&self) {
        if self.generations.is_empty() {
            return;
        }
        let first_gen = &self.generations[0];
        let last_gen = self.generations.last().unwrap();

        // Best generation by accuracy.
        let mut best_gen: Option<&GenerationRecord> = None;
        let mut best_metric = f64::NEG_INFINITY;
        for g in &self.generations {
            if let Some(acc) = g.metrics.get("accuracy").and_then(acc_to_f64) {
                if acc > best_metric {
                    best_metric = acc;
                    best_gen = Some(g);
                }
            }
        }

        let evolution_text = match (
            first_gen.metrics.get("accuracy").and_then(acc_to_f64),
            last_gen.metrics.get("accuracy").and_then(acc_to_f64),
        ) {
            (Some(first_acc), Some(last_acc)) => {
                let gain = last_acc - first_acc;
                format!("{first_acc:.2}% → {last_acc:.2}% ({gain:+.2}%)")
            }
            _ => "N/A".to_string(),
        };

        let successful = self.generations.iter().filter(|g| g.success).count();
        let best_gen_label = best_gen
            .map(|g| g.gen_num.to_string())
            .unwrap_or_else(|| "N/A".to_string());

        let growth_lines = last_gen.agent_stats.lines as i64 - first_gen.agent_stats.lines as i64;
        let growth_bytes = last_gen.agent_stats.size as i64 - first_gen.agent_stats.size as i64;

        let summary = format!(
            "## Summary Statistics\n\n\
**Total Generations**: {total}\n\
**Successful Executions**: {successful}\n\
**Best Performance**: Generation {best_label} ({best_metric:.2}% accuracy)\n\n\
**Evolution**:\n\
- {evolution_text}\n\n\
**Code Growth**:\n\
- Initial: {first_lines} lines ({first_size} bytes)\n\
- Final: {last_lines} lines ({last_size} bytes)\n\
- Growth: {growth_lines} lines ({growth_bytes} bytes)\n",
            total = self.generations.len(),
            successful = successful,
            best_label = best_gen_label,
            best_metric = best_metric,
            evolution_text = evolution_text,
            first_lines = first_gen.agent_stats.lines,
            first_size = commas_u64(first_gen.agent_stats.size),
            last_lines = last_gen.agent_stats.lines,
            last_size = commas_u64(last_gen.agent_stats.size),
            growth_lines = growth_lines,
            growth_bytes = commas_i64_signed(growth_bytes),
        );

        if let Ok(mut existing) = std::fs::read_to_string(&self.context_path) {
            existing.push_str(&summary);
            let _ = std::fs::write(&self.context_path, existing);
        }
    }

    fn get_agent_stats(&self, agent_path: &str) -> AgentStats {
        match std::fs::read_to_string(agent_path) {
            Ok(content) => {
                let size = std::fs::metadata(agent_path)
                    .map(|m| m.len())
                    .unwrap_or(content.len() as u64);
                AgentStats {
                    size,
                    lines: count_readlines(&content),
                }
            }
            Err(_) => AgentStats { size: 0, lines: 0 },
        }
    }

    fn extract_metrics(&self, gen_dir: &str) -> Map<String, Value> {
        let mut metrics = Map::new();

        // Priority 1: results.json — all top-level scalar values, in order.
        let results_path = format!("{gen_dir}/results.json");
        if Path::new(&results_path).exists() {
            if let Some(Value::Object(data)) = safe_load_json(&results_path, default_max_bytes()) {
                for (key, value) in data.iter() {
                    if is_scalar(value) {
                        metrics.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        // Priority 2: detailed_results.json (only if no metrics yet).
        let detailed_path = format!("{gen_dir}/detailed_results.json");
        if metrics.is_empty() && Path::new(&detailed_path).exists() {
            if let Some(Value::Object(data)) = safe_load_json(&detailed_path, default_max_bytes()) {
                for (key, value) in data.iter() {
                    if is_scalar(value) {
                        metrics.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        // Priority 3: parse stdout (only if no metrics yet).
        let stdout_path = format!("{gen_dir}/target_agent_stdout.log");
        if metrics.is_empty() && Path::new(&stdout_path).exists() {
            for (k, v) in self.parse_stdout_metrics(&stdout_path) {
                metrics.insert(k, v);
            }
        }

        metrics
    }

    fn parse_stdout_metrics(&self, stdout_path: &str) -> Vec<(String, Value)> {
        let mut metrics: Vec<(String, Value)> = Vec::new();
        let content = match safe_read_file(stdout_path, default_max_bytes()) {
            Some(c) => c,
            None => return metrics,
        };

        let patterns: [(&str, Vec<&str>); 4] = [
            (
                "accuracy",
                vec![
                    r"accuracy[:\s=]+(\d+\.?\d*)\s*%?",
                    r"final\s+accuracy[:\s=]+(\d+\.?\d*)\s*%?",
                    r"test\s+accuracy[:\s=]+(\d+\.?\d*)\s*%?",
                ],
            ),
            (
                "validation",
                vec![r"validation[:\s=]+(\d+\.?\d*)", r"val[:\s=]+(\d+\.?\d*)"],
            ),
            (
                "correct",
                vec![r"(\d+)\s*/\s*\d+\s+correct", r"correct[:\s=]+(\d+)"],
            ),
            (
                "total",
                vec![r"\d+\s*/\s*(\d+)\s+(?:questions|samples|total)"],
            ),
        ];

        for (metric_name, pattern_list) in patterns {
            for pattern in pattern_list {
                let re = regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .unwrap();
                if let Some(caps) = re.captures(&content) {
                    if let Some(m) = caps.get(1) {
                        if let Ok(value) = m.as_str().parse::<f64>() {
                            metrics.push((metric_name.to_string(), number(value)));
                            break;
                        }
                    }
                }
            }
        }
        metrics
    }

    fn extract_insights(&self, improvement_path: &str) -> Vec<String> {
        let content = match safe_read_file(improvement_path, default_max_bytes()) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let bullet_re = regex::RegexBuilder::new(r"^[-*]\s+(.+)$")
            .multi_line(true)
            .build()
            .unwrap();
        let numbered_re = regex::RegexBuilder::new(r"^\d+\.\s+(.+)$")
            .multi_line(true)
            .build()
            .unwrap();

        let mut all: Vec<String> = Vec::new();
        for caps in bullet_re.captures_iter(&content) {
            all.push(caps[1].to_string());
        }
        for caps in numbered_re.captures_iter(&content) {
            all.push(caps[1].to_string());
        }

        all.into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| s.chars().count() > 20 && !s.ends_with(':'))
            .take(5)
            .collect()
    }

    /// Attempt an LLM-generated change summary. The Rust port has no LLM SDK wired,
    /// so the agent runner errs and this returns `None` (matching the Python
    /// golden tests that patch `_generate_llm_summary` to `None`).
    fn generate_llm_summary(&self, gen_num: i64) -> Option<String> {
        if gen_num == 1 {
            return None;
        }
        let tmp =
            std::env::temp_dir().join(format!("sia-summary-{}-{}", std::process::id(), gen_num));
        std::fs::create_dir_all(&tmp).ok()?;
        let summary_file = tmp.join("summary.txt");
        let prompt = format!(
            "Summarize the changes for generation {gen_num}. Write to {}",
            summary_file.display()
        );
        let result = run_agent(
            &self.meta_model,
            &self.cfg.context_summary_max_turns.to_string(),
            &prompt,
            &tmp.to_string_lossy(),
            &self.agent_impl,
            None,
        );
        let out = match result {
            Ok(_) => {
                safe_read_file(&summary_file, default_max_bytes()).map(|s| s.trim().to_string())
            }
            Err(_) => None,
        };
        let _ = std::fs::remove_dir_all(&tmp);
        out.filter(|s| !s.is_empty())
    }

    #[allow(clippy::too_many_arguments)]
    fn format_generation_entry(
        &self,
        gen_num: i64,
        gen_data: &GenData,
        stats: &AgentStats,
        deltas: Option<(f64, i64)>,
        metrics: &Map<String, Value>,
        insights: &[String],
        llm_summary: Option<&str>,
    ) -> String {
        let status = if gen_data.success {
            "✓ SUCCESS"
        } else {
            "✗ FAILED"
        };

        let mut entry = format!(
            "## Generation {gen_num}\n\n\
**Status**: {status}\n\
**Timestamp**: {timestamp}\n\
**Duration**: {duration:.1}s\n\n\
### Target Agent Changes\n",
            gen_num = gen_num,
            status = status,
            timestamp = gen_data.timestamp,
            duration = gen_data.duration,
        );

        if gen_num == 1 {
            entry.push_str(&format!(
                "- Initial agent created by meta-agent\n- File size: {size} bytes\n- Lines of code: {lines}\n",
                size = commas_u64(stats.size),
                lines = stats.lines,
            ));
        } else {
            let (size_pct, lines_delta) = deltas.unwrap_or((0.0, 0));
            let delta_size_str = if size_pct > 0.0 {
                format!("+{size_pct:.1}%")
            } else {
                format!("{size_pct:.1}%")
            };
            let delta_lines_str = if lines_delta > 0 {
                format!("+{lines_delta}")
            } else {
                format!("{lines_delta}")
            };
            entry.push_str(&format!(
                "- Modified by feedback agent\n\
- File size: {size} bytes ({delta_size_str})\n\
- Lines: {lines} ({delta_lines_str} lines)\n",
                size = commas_u64(stats.size),
                delta_size_str = delta_size_str,
                lines = stats.lines,
                delta_lines_str = delta_lines_str,
            ));
            if !insights.is_empty() {
                entry.push_str("- Key changes from improvement.md:\n");
                for insight in insights.iter().take(3) {
                    let limit = self.cfg.insight_preview_limit;
                    let insight_text = if insight.chars().count() > limit {
                        let truncated: String = insight.chars().take(limit).collect();
                        format!("{truncated}...")
                    } else {
                        insight.clone()
                    };
                    entry.push_str(&format!("  * {insight_text}\n"));
                }
            }
        }

        if let Some(llm) = llm_summary {
            entry.push_str(&format!("\n### Evolution Summary (LLM Analysis)\n{llm}\n"));
        }

        entry.push_str(&format!(
            "\n### Execution Summary\n\
- Execution status: {status}\n\
- Output format: {execution_type}\n\n\
### Performance Metrics\n",
            status = status,
            execution_type = gen_data.execution_type,
        ));

        if metrics.is_empty() {
            entry.push_str("- No structured metrics found\n");
        } else {
            for (key, value) in metrics.iter() {
                entry.push_str(&format!("- {key}: {}\n", format_metric_value(value)));
            }
        }

        // Changes vs previous generation.
        if gen_num > 1 {
            if let Some(prev) = self.generations.last() {
                let prev_metrics = &prev.metrics;
                let mut changes: Vec<String> = Vec::new();
                for (key, current) in metrics.iter() {
                    if let Some(previous) = prev_metrics.get(key) {
                        if let (Some(c), Some(p)) = (numericish(current), numericish(previous)) {
                            let delta = c - p;
                            changes.push(format!("- {key}: {delta:+.2}"));
                        }
                    }
                }
                if !changes.is_empty() {
                    entry.push_str("\n### Changes vs Previous Generation\n");
                    entry.push_str(&changes.join("\n"));
                    entry.push('\n');
                }
            }
        }

        entry
    }
}

fn is_scalar(v: &Value) -> bool {
    matches!(v, Value::Number(_) | Value::String(_) | Value::Bool(_))
}

fn number(f: f64) -> Value {
    serde_json::Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Format a metric value as the Python f-string would: floats with `.2f`, ints/strings
/// verbatim, bools as `True`/`False`.
fn format_metric_value(v: &Value) -> String {
    match v {
        // serde_json keeps integers and floats distinct: a decimal literal is f64,
        // an integer literal is i64/u64. Mirror Python's `isinstance(value, float)`.
        Value::Number(n) if n.is_f64() => format!("{:.2}", n.as_f64().unwrap()),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        other => other.to_string(),
    }
}

/// Numeric value for delta math: numbers as f64; strings parsed (stripping `%`).
fn numericish(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_percentish(s),
        _ => None,
    }
}

/// Accuracy value for best/evolution: numbers as f64; strings parsed (stripping `%`).
fn acc_to_f64(v: &Value) -> Option<f64> {
    numericish(v)
}
