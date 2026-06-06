//! Differential-parity helper. Reads a JSON request on stdin and prints the Rust
//! implementation's output for one operation, so `scripts/parity_check.py` can
//! diff it against the reference Python implementation. See issue #29.
//!
//! Usage: `sia-parity <mode>` where mode is one of:
//!   json-dumps | meta-prompt | feedback-prompt | feedback-context | load-exec
//!
//! Required fields are accessed strictly: a missing/ill-typed required field exits
//! non-zero rather than substituting a default, so the parity matrix can never
//! accidentally prove parity for a degenerate (silently-defaulted) input.

use std::io::Read;
use std::process::exit;

use serde_json::Value;
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

fn fail(msg: String) -> ! {
    eprintln!("sia-parity: {msg}");
    exit(2);
}

fn req<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k)
        .unwrap_or_else(|| fail(format!("missing required field '{k}'")))
}

fn req_str<'a>(v: &'a Value, k: &str) -> &'a str {
    req(v, k)
        .as_str()
        .unwrap_or_else(|| fail(format!("field '{k}' must be a string")))
}

fn req_i64(v: &Value, k: &str) -> i64 {
    req(v, k)
        .as_i64()
        .unwrap_or_else(|| fail(format!("field '{k}' must be an integer")))
}

fn req_bool(v: &Value, k: &str) -> bool {
    req(v, k)
        .as_bool()
        .unwrap_or_else(|| fail(format!("field '{k}' must be a boolean")))
}

/// Optional string field (absent => None). Used only for genuinely optional inputs.
fn opt_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    match v.get(k) {
        None | Some(Value::Null) => None,
        Some(x) => Some(
            x.as_str()
                .unwrap_or_else(|| fail(format!("field '{k}' must be a string"))),
        ),
    }
}

fn task_files(v: &Value) -> TaskFiles {
    TaskFiles::new(
        req_str(v, "sample_task_descriptions"),
        req_str(v, "reference_target_agent_py"),
        req(v, "sample_agent_execution").clone(),
        req_str(v, "task_md"),
    )
}

fn opt_provider(v: &Value) -> Option<Provider> {
    match v.get("provider") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(load_provider(name).expect("load provider")),
        Some(_) => fail("field 'provider' must be a string provider name".to_string()),
    }
}

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| fail("mode arg required".to_string()));
    let req_v = read_stdin();

    match mode.as_str() {
        // The whole request body is the value to serialize.
        "json-dumps" => print!("{}", sia::pyjson::dumps_indent2(&req_v)),
        "meta-prompt" => {
            let tf = task_files(req(&req_v, "task_files"));
            let provider = opt_provider(&req_v);
            let out = build_meta_prompt(
                &tf,
                req_str(&req_v, "task_model"),
                req_str(&req_v, "working_dir"),
                provider.as_ref(),
                opt_str(&req_v, "reference_dir"),
            );
            print!("{out}");
        }
        "feedback-prompt" => {
            let tf = task_files(req(&req_v, "task_files"));
            let provider = opt_provider(&req_v);
            let out = build_feedback_prompt(
                req_i64(&req_v, "current_gen"),
                req_i64(&req_v, "max_gen"),
                &tf,
                req_str(&req_v, "agent_py"),
                req_str(&req_v, "task"),
                req_str(&req_v, "execution_status"),
                req_str(&req_v, "execution_section"),
                req_str(&req_v, "run_dir"),
                req_str(&req_v, "next_gen_dir"),
                req_str(&req_v, "previous_gens"),
                req_str(&req_v, "task_model"),
                provider.as_ref(),
                opt_str(&req_v, "requirements_dir"),
            );
            print!("{out}");
        }
        "feedback-context" => {
            let tf = task_files(req(&req_v, "task_files"));
            let (status, section) = build_feedback_context(
                req_i64(&req_v, "current_gen"),
                req_str(&req_v, "gen_dir"),
                req_str(&req_v, "dataset_dir"),
                req_bool(&req_v, "success"),
                req_str(&req_v, "error_msg"),
                req_str(&req_v, "stdout"),
                req_str(&req_v, "stderr"),
                req_str(&req_v, "stdout_log_file"),
                &tf,
                &Config::default(),
            );
            print!(
                "{}",
                serde_json::to_string(&serde_json::json!({"status": status, "section": section}))
                    .unwrap()
            );
        }
        "load-exec" => {
            let (data, is_multi) =
                load_agent_execution(req_str(&req_v, "gen_dir"), &Config::default());
            print!(
                "{}",
                serde_json::to_string(&serde_json::json!({"data": data, "is_multi": is_multi}))
                    .unwrap()
            );
        }
        other => fail(format!("unknown mode: {other}")),
    }
}
