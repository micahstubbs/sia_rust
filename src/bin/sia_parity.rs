//! Differential-parity helper. Reads a JSON request on stdin and prints the Rust
//! implementation's output for one operation, so `scripts/parity_check.py` can
//! diff it against the reference Python implementation. See issue #29.
//!
//! Usage: `sia-parity <mode>` where mode is one of:
//!   json-dumps | meta-prompt | feedback-prompt | feedback-context | load-exec

use std::io::Read;

use serde_json::{json, Value};
use sia::config::Config;
use sia::orchestrator::{build_feedback_context, load_agent_execution};
use sia::prompts::{build_feedback_prompt, build_meta_prompt};
use sia::providers::{load_provider, Provider};
use sia::TaskFiles;

fn read_stdin() -> Value {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .expect("read stdin");
    serde_json::from_str(&buf).expect("parse stdin JSON")
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn task_files(v: &Value) -> TaskFiles {
    TaskFiles::new(
        s(v, "sample_task_descriptions"),
        s(v, "reference_target_agent_py"),
        v.get("sample_agent_execution")
            .cloned()
            .unwrap_or(json!({})),
        s(v, "task_md"),
    )
}

fn opt_provider(v: &Value) -> Option<Provider> {
    match v.get("provider") {
        Some(Value::String(name)) => Some(load_provider(name).expect("load provider")),
        _ => None,
    }
}

fn main() {
    let mode = std::env::args().nth(1).expect("mode arg");
    let req = read_stdin();

    match mode.as_str() {
        "json-dumps" => {
            print!("{}", sia::pyjson::dumps_indent2(&req));
        }
        "meta-prompt" => {
            let tf = task_files(req.get("task_files").unwrap_or(&json!({})));
            let provider = opt_provider(&req);
            let reference_dir = req.get("reference_dir").and_then(|x| x.as_str());
            let out = build_meta_prompt(
                &tf,
                s(&req, "task_model"),
                s(&req, "working_dir"),
                provider.as_ref(),
                reference_dir,
            );
            print!("{out}");
        }
        "feedback-prompt" => {
            let tf = task_files(req.get("task_files").unwrap_or(&json!({})));
            let provider = opt_provider(&req);
            let out = build_feedback_prompt(
                req.get("current_gen").and_then(|x| x.as_i64()).unwrap_or(0),
                req.get("max_gen").and_then(|x| x.as_i64()).unwrap_or(0),
                &tf,
                s(&req, "agent_py"),
                s(&req, "task"),
                s(&req, "execution_status"),
                s(&req, "execution_section"),
                s(&req, "run_dir"),
                s(&req, "next_gen_dir"),
                s(&req, "previous_gens"),
                s(&req, "task_model"),
                provider.as_ref(),
                req.get("requirements_dir").and_then(|x| x.as_str()),
            );
            print!("{out}");
        }
        "feedback-context" => {
            let tf = task_files(req.get("task_files").unwrap_or(&json!({})));
            let (status, section) = build_feedback_context(
                req.get("current_gen").and_then(|x| x.as_i64()).unwrap_or(1),
                s(&req, "gen_dir"),
                s(&req, "dataset_dir"),
                req.get("success")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                s(&req, "error_msg"),
                s(&req, "stdout"),
                s(&req, "stderr"),
                s(&req, "stdout_log_file"),
                &tf,
                &Config::default(),
            );
            print!(
                "{}",
                serde_json::to_string(&json!({"status": status, "section": section})).unwrap()
            );
        }
        "load-exec" => {
            let (data, is_multi) = load_agent_execution(s(&req, "gen_dir"), &Config::default());
            print!(
                "{}",
                serde_json::to_string(&json!({"data": data, "is_multi": is_multi})).unwrap()
            );
        }
        other => {
            eprintln!("unknown mode: {other}");
            std::process::exit(2);
        }
    }
}
