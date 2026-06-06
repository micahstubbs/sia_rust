//! Prompt builders for the meta-agent and feedback-agent. Port of `sia/prompts.py`.
//!
//! The exact text is product-critical and locked by golden-master tests; it is
//! reproduced byte-for-byte here. `format!` does single-pass substitution (like a
//! Python f-string), so injected bodies are never re-scanned for placeholders.

use crate::providers::Provider;
use crate::task_files::TaskFiles;

/// `json.dumps(value, indent=2)` (ensure_ascii=True) — non-ASCII task data (e.g.
/// LawBench, which is Chinese) is escaped exactly as CPython does.
fn json_pretty(value: &serde_json::Value) -> String {
    crate::pyjson::dumps_indent2(value)
}

/// The reference paragraph of the meta prompt.
fn reference_section(task_files: &TaskFiles, reference_dir: Option<&str>) -> String {
    match reference_dir {
        None => format!(
            "Here is a sample target_agent.py showing the complete implementation pattern \
             (READ THE ENTIRE FILE):\n{}",
            task_files.reference_target_agent_py
        ),
        Some(reference_dir) => format!(
            "Your reference agent implementation has been placed in your working directory ({reference_dir}). \
It may span multiple files. READ IT YOURSELF with your file tools (Read/Glob/Grep) — study the \
entrypoint and any helper modules — then write your target_agent.py in the same directory.\n\
If your target_agent.py needs third-party packages, list them (one per line) in a requirements.txt \
in your working directory; they are installed before the target agent runs."
        ),
    }
}

/// Build the meta-agent prompt for creating the initial target agent.
pub fn build_meta_prompt(
    task_files: &TaskFiles,
    task_model: &str,
    working_dir: &str,
    provider: Option<&Provider>,
    reference_dir: Option<&str>,
) -> String {
    let reference_section = reference_section(task_files, reference_dir);
    let sample_execution = json_pretty(&task_files.sample_agent_execution);
    let base = format!(
        r#"You are a meta-agent. Your task is to create a target agent which can execute a task. Go ahead and create a target_agent.py for the target agent, which in turn can solve the given task.

Here is the FULL TASK SPECIFICATION that your target_agent.py will need to solve:
{task_md}

Here are a couple of sample task descriptions which the target agent has to solve:
{sample_task_descriptions}

{reference_section}

Here is a sample agent execution trajectory:
{sample_execution}

CRITICAL RULES - FOLLOW EXACTLY:

1. The current working directory is {working_dir}. Create the target_agent.py in the current working directory itself.

2. The target_agent.py MUST accept two command-line arguments:
   - --dataset_dir: Absolute path to the dataset directory (READ-ONLY, provided at runtime)
   - --working_dir: Absolute path to the working directory (READ-WRITE, provided at runtime)

3. CRITICAL: The target_agent.py must INCLUDE these paths in the prompt it sends to {task_model}. {task_model} MUST be explicitly told:
   - Where the dataset directory is located (the exact path from --dataset_dir)
   - Where the working directory is located (the exact path from --working_dir)
   - That it can ONLY READ from the dataset directory
   - That it can READ from and WRITE to the working directory

   DO NOT let {task_model} search for data in random locations. The prompt must say: "The dataset is at: <actual_dataset_dir_path>"

4. The target agent can ONLY read from the dataset directory provided via --dataset_dir, and can ONLY write to the working directory specified by --working_dir. It must NOT access any other directories on the filesystem.

5. EXECUTION LOGGING - CRITICAL:

   The target_agent.py must log its execution trajectory properly. The format depends on the task type:

   **FOR TASKS WITH MULTIPLE INDEPENDENT SAMPLES** (e.g., GPQA with 198 questions, multiple test cases):
   - Create a folder: agent_execution/ in the working directory
   - Save each sample separately: execution_q0.json, execution_q1.json, execution_q2.json, etc.
   - Each file contains the complete trajectory for that ONE sample only
   - Files must be named sequentially: execution_q0.json, execution_q1.json, ...

   **FOR TASKS WITH SINGLE EXECUTION** (e.g., building one ML model, analyzing one dataset):
   - Save to a single file: agent_execution.json in the working directory
   - File contains the complete execution trajectory

   **HOW TO DETERMINE WHICH FORMAT**:
   - Read the task description carefully
   - If it mentions "independent items", "dataset with multiple records to process separately"
     → Use multi-trajectory (folder with multiple files)
   - If it's about "build a model", "analyze the dataset", "create one solution", "optimize one system"
     → Use single-trajectory (one JSON file)

   **FORMAT REQUIREMENTS** (both formats):
   - Use the same format as the sample agent execution trajectory provided above
   - Include all messages, tool calls, and their results
   - Ensure valid JSON (properly close all arrays/objects)
   - Make sure to properly close the JSON file(s) to avoid corruption

6. Do NOT attempt to write to or modify files inside the dataset directory. It is READ-ONLY.
7. The target_agent.py should use only the "{task_model}" model when invoking the language model (do not use any other model).
8. DO NOT hardcode any specific dataset paths in the target_agent.py code. The paths will be provided at runtime via command-line arguments and MUST be passed to {task_model} in the prompt.

Example invocation (paths will vary at runtime):
    python target_agent.py --dataset_dir /path/to/dataset --working_dir /path/to/working
"#,
        task_md = task_files.task_md,
        sample_task_descriptions = task_files.sample_task_descriptions,
        reference_section = reference_section,
        sample_execution = sample_execution,
        working_dir = working_dir,
        task_model = task_model,
    );

    match provider {
        Some(p) if p.client_kind == "openai" => build_target_client_setup(p, task_model) + &base,
        _ => base,
    }
}

/// Prompt block telling the meta-agent how to reach an OpenAI-compatible target model.
pub fn build_target_client_setup(provider: &Provider, task_model: &str) -> String {
    let base_url = provider.base_url.as_deref().unwrap_or("None");
    format!(
        r#"=== TARGET MODEL CLIENT SETUP (OpenAI-compatible provider: {name}) ===

The target model "{task_model}" is served by an OpenAI-compatible API. The reference
target_agent.py shown below may use a different SDK (e.g. the Gemini SDK) — you MUST
refactor your target_agent.py to use the `openai` SDK configured for this provider
(do NOT use the anthropic or google SDK):

    import os
    from openai import OpenAI

    client = OpenAI(
        base_url="{base_url}",
        api_key=os.environ["{api_key_env}"],
    )

Call client.chat.completions.create(model="{task_model}", ...) using OpenAI-style
messages (and OpenAI function calling / response_format where the reference uses
structured output). Do NOT compute a dollar cost: per-provider pricing is unknown, so
set any cost field to 0 (token counts from the API response are still fine to record).

"#,
        name = provider.name,
        task_model = task_model,
        base_url = base_url,
        api_key_env = provider.api_key_env,
    )
}

/// Build the feedback agent prompt for improving the target agent.
#[allow(clippy::too_many_arguments)]
pub fn build_feedback_prompt(
    _current_gen: i64,
    _max_gen: i64,
    task_files: &TaskFiles,
    agent_py: &str,
    task: &str,
    execution_status: &str,
    execution_section: &str,
    run_dir: &str,
    next_gen_dir: &str,
    previous_gens: &str,
    task_model: &str,
    provider: Option<&Provider>,
    requirements_dir: Option<&str>,
) -> String {
    let current_gen = _current_gen;
    let context_md_path = format!("{run_dir}/context.md");

    let mut base = format!(
        r#"You are an expert AI Engineer analyzing agent scaffolds for iterative improvement.

**GENERATION CONTEXT**:
- Current generation: {current_gen}
- Previous generations: {previous_gens}
- Evolution history: {context_md_path}

**BEFORE ANALYZING - READ THE FULL HISTORY**:
1. Read {context_md_path} to understand:
   - What improvements were tried in each previous generation
   - Performance trends across generations
   - What worked and what didn't work
2. Review previous improvement.md files from earlier generations if helpful
3. Don't repeat failed approaches from earlier generations
4. Build upon successful patterns that improved performance

---

**SAMPLE TASK DESCRIPTIONS**:
```
{sample_task_descriptions}
```

**CURRENT TARGET AGENT** (Generation {current_gen}):
```python
{agent_py}
```

**TASK WORKED ON**:
```
{task}
```

**EXECUTION STATUS**:
```
{execution_status}
```

**EXECUTION LOGS**:
{execution_section}

---

**YOUR TASK**:

You must create exactly TWO files in {next_gen_dir}/:
1. improvement.md - Analysis and improvement plan
2. target_agent.py - The improved agent implementation

Follow these steps:

**STEP 1: Analyze the execution**:
   - For multi-trajectory: Look for patterns across all trajectories
   - For single-trajectory: Analyze the full execution flow
   - Identify what worked well and what failed
   - Check for consistency and robustness

**STEP 2: Review evolution history**:
   - Read context.md to see the full evolution
   - Understand what was tried in previous generations
   - Build upon successful patterns
   - Avoid repeating failed approaches

**STEP 3: Write improvement.md**:
   - MUST save to: {next_gen_dir}/improvement.md
   - Document your analysis and planned improvements
   - Focus on structural improvements to the agent scaffold
   - Make the agent more robust and generalizable
   - Don't optimize for this specific task
   - Reference insights from previous generations if applicable

**STEP 4: Create improved target_agent.py**:
   - MUST save to: {next_gen_dir}/target_agent.py
   - Implement the improvements documented in improvement.md
   - Apply all the planned improvements from step 3
   - Do not create or modify any other files besides these two

**RULES**:
- Focus on agent structure, not task-specific optimizations
- Make the agent work well across diverse task types (see sample task descriptions)
- If execution failed, fix the root cause
- If multi-trajectory: ensure each trajectory is properly isolated and logged
- Consider error handling, logging mechanisms, and robustness
- Build upon successful patterns from previous generations (check context.md)
- If execution log shows errors or is incomplete, suggest improvements to ensure proper logging

NOTE: The agent execution log may be incomplete or contain errors if the target agent crashed. If you see an "error" field, focus on making the agent more robust to prevent such failures.
"#,
        current_gen = current_gen,
        previous_gens = previous_gens,
        context_md_path = context_md_path,
        sample_task_descriptions = task_files.sample_task_descriptions,
        agent_py = agent_py,
        task = task,
        execution_status = execution_status,
        execution_section = execution_section,
        next_gen_dir = next_gen_dir,
    );

    if let Some(requirements_dir) = requirements_dir {
        base += &format!(
            "\nNOTE ON DEPENDENCIES: You may also create or edit a requirements.txt in {requirements_dir} \
(one package per line) to declare third-party packages your target_agent.py needs; they are \
installed before the target agent runs. This is the one exception to the \"only two files\" rule above.\n"
        );
    }

    match provider {
        Some(p) if p.client_kind == "openai" => build_target_client_setup(p, task_model) + &base,
        _ => base,
    }
}
