# Architecture

SIA coordinates three AI agents in a loop. Each generation, the system inspects the previous attempt, rewrites the agent, and runs it again.

## The three agents

1. **Meta-Agent** — Reads the task description and generates the initial Target Agent tailored to the task.
2. **Target Agent** — Attempts to complete the task and records its actions and results.
3. **Feedback / Improvement Agent** — Reviews the Target Agent's execution logs, identifies improvements, and rewrites the Target Agent for the next generation.

## What happens during a run

**Generation 1:**
- Meta-agent reads the task and writes `target_agent.py`
- Target agent executes the task and logs to `agent_execution.json`
- Feedback agent analyzes the run and writes an improved agent for Gen 2

**Generation 2 through N:**
- The current generation's target agent executes the task
- The feedback agent analyzes and produces the next generation
- Continues until `--max_gen` is reached

**Output:**
- All artifacts saved under `runs/run_{run_id}/gen_{n}/`
- Each generation has its own `target_agent.py` and `agent_execution.json`
- Improvement notes land in `improvement.md` (gen 2 onwards)

## Target execution (the `TargetExecutor` seam)

Target agents are LLM-authored **Python** ML programs; SIA-Rust runs them, it does
not rewrite them. Per ADR-0001 ("deprecate the Python bridge", issue #138), how a
target agent is executed sits behind a Rust-native **execution seam** rather than
being hard-wired into the generation loop.

`src/target_exec.rs` defines a `TargetExecutor` trait — one method that runs a
single generation and returns the existing `(success, stdout, stderr, error_msg)`
tuple — with two strategies:

- **`PythonVenvExecutor`** (default): today's behavior. It delegates to
  `orchestrator::run_target_agent`, covering both the plain per-generation venv
  subprocess (`python -u target_agent.py --dataset_dir … --working_dir …`) and the
  Docker-sandboxed path. Byte-for-byte identical to the pre-seam code path.
- **`NativeExecutor`** (scaffold, `TODO(#138)`): the intended Python-bridge-free
  path — a capability-confined direct subprocess that drops the per-generation
  uv/pip venv bridge, confines I/O through the `sandbox` allow-list, and streams
  output through the same `stream_to_log` contract. **Not** wired as default; it
  returns a clear "not yet implemented" error if invoked.

The generation loop (`orchestrator::run_generation_with`) keeps its existing
injectable `target_fn` closure seam (tests depend on it); `target_exec::target_fn_for`
adapts any `TargetExecutor` into that closure, and `run.rs` defaults to
`PythonVenvExecutor`. This lets the bridge be hardened or replaced **incrementally**
without changing the loop, its signatures, or the parity-checked surfaces.

**Roadmap:** flesh out `NativeExecutor` behind the same trait, add offline tests for
it via the injectable process-runner seam, then flip the default once it reaches
parity — all without touching `run_generation_with`.

## Directory layout

```
sia/
├── sia/
│   ├── orchestrator.py             # Main orchestration logic
│   ├── context_manager.py          # Run/context tracking
│   ├── util.py                     # Agent runner utilities
│   ├── prepare_mlebench_dataset.py # MLE-Bench dataset preparation
│   └── tasks/                      # Bundled with the wheel
│       ├── _shared/
│       │   ├── reference_target_agent.py
│       │   └── sample_agent_execution.json
│       └── {task-id}/              # gpqa, lawbench, longcot-chess, spaceship-titanic
│           ├── data/
│           │   ├── public/         # Public dataset
│           │   │   ├── task.md         # Task description
│           │   │   └── *.csv           # Data files
│           │   └── private/        # Held-out evaluation data
│           └── reference/
│               ├── SAMPLE_TASK_DESCRIPTIONS.md
│               └── reference_target_agent.py
└── runs/                           # Generated during execution
    └── run_{id}/
        ├── venv/                   # Isolated Python environment per run
        └── gen_{n}/                # Each generation's artifacts
            ├── target_agent.py
            ├── agent_execution.json
            └── improvement.md      # gen 2 onwards
```

## Customizing prompts

The two prompts that drive self-improvement live in [`sia/orchestrator.py`](../sia/orchestrator.py):

- `META_AGENT_PROMPT` — controls how the initial Target Agent is created
- `FEEDBACK_AGENT_PROMPT` — controls how improvements are suggested
