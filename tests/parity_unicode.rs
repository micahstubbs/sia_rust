//! Regression guard for the ensure_ascii parity fix (#29): non-ASCII task data
//! embedded via json.dumps must be escaped as \uXXXX, matching CPython. The full
//! Python⇄Rust differential is in scripts/parity_check.py.

use serde_json::json;
use sia::prompts::build_meta_prompt;
use sia::TaskFiles;

const CJK: &str = "汉字"; // 汉 = U+6C49, 字 = U+5B57
const CJK_ESCAPED: &str = "\\u6c49\\u5b57";

#[test]
fn test_meta_prompt_escapes_cjk_sample_execution() {
    // CJK is embedded only via json.dumps(sample_agent_execution).
    let tf = TaskFiles::new(
        "desc",
        "ref",
        json!([{"role": "user", "content": CJK}]),
        "# Task",
    );
    let prompt = build_meta_prompt(&tf, "model", "/work", None, None);
    assert!(
        prompt.contains(CJK_ESCAPED),
        "CJK should be \\u-escaped in the json.dumps block"
    );
    assert!(
        !prompt.contains(CJK),
        "raw CJK must not appear in the json.dumps block"
    );
}

#[test]
fn test_task_md_field_stays_raw() {
    // task_md is embedded as a raw f-string (not json.dumps), so CJK stays raw there.
    let tf = TaskFiles::new("desc", "ref", json!({}), "# 任务\n预测罪名");
    let prompt = build_meta_prompt(&tf, "model", "/work", None, None);
    assert!(
        prompt.contains("# 任务"),
        "raw task_md field should keep its original bytes"
    );
}
