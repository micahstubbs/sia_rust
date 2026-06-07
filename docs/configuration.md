# Configuration

Full reference for SIA's agent **profiles**, **providers**, and command-line arguments.

## Command-line arguments

SIA has two sub-commands: **`sia run`** (the self-improvement loop) and **`sia web`**
(the runs visualizer, see [Visualizing runs](#visualizing-runs)). For backward
compatibility, `sia <flags>` with no sub-command is treated as `sia run <flags>`.

### `sia run`

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--task` | one of | — | Name of a bundled task: `gpqa`, `lawbench`, `longcot-chess`, `spaceship-titanic` |
| `--task_dir` | one of | — | Path to an external task directory (mutually exclusive with `--task`) |
| `--max_gen` | no | `3` | Number of self-improvement generations |
| `--run_id` | no | `1` | Unique run identifier |
| `--meta-agent-profile` | no | `default-meta` | Profile for the meta/feedback agent (name or path to a `.json`) |
| `--target-agent-profile` | no | `default-target` | Profile for the target agent (name or path to a `.json`) |
| `--sandbox` | no | `none` | Target-agent isolation: `none` or `docker` |
| `--runs-dir` | no | `./runs` | Directory where run artifacts are written and served from (also honors `$SIA_RUNS_DIR`; the flag wins) |
| `--no-web` | no | off | Don't auto-start the live dashboard during the run |
| `--web-host` | no | `127.0.0.1` | Bind host for the live dashboard |
| `--web-port` | no | `8000` | Bind port for the live dashboard |

### `sia web`

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--runs-dir` | no | `./runs` | Directory of runs to visualize |
| `--host` | no | `127.0.0.1` | Bind host |
| `--port` | no | `8000` | Bind port |
| `--no-browser` | no | off | Don't open a browser window automatically |

There are two agent roles, each selected by a profile:

- the **meta/feedback agent** runs *inside* SIA via an agent impl (`claude` / `openhands` /
  `pydantic-ai`) — selected with `--meta-agent-profile`;
- the **target agent** is *generated code* SIA never runs as an engine — its model/provider come
  from `--target-agent-profile`, and the meta-agent refactors that profile's `agent_reference`
  (the seed code) to the provider's SDK and iteratively improves it.

## Profiles and providers

Configuration is **declarative JSON** you can extend without touching code.

### Provider — an endpoint + credentials

```jsonc
// sia/defaults/providers/nebius.json
{
  "provider_id": "nebius",                                   // stable id (also the filename stem)
  "name": "Nebius Token Factory",                            // human-readable display name
  "client_kind": "openai",                                   // anthropic | openai | google
  "base_url": "https://api.tokenfactory.us-central1.nebius.com/v1/",
  "api_key_env": "NEBIUS_API_KEY"
}
```

Bundled providers: `anthropic`, `gemini`, `openai`, `together`, `nebius`, `tinker`.

### Profiles — one per agent role

A **meta-agent profile** bundles `(agent_impl, model, provider)`:

```jsonc
// sia/defaults/profiles/kimi-nebius-meta.json
{
  "profile_id": "kimi-nebius-meta", // stable id (also the value you pass to --meta-agent-profile)
  "name": "Kimi K2.6 on Nebius",    // human-readable display name
  "agent_impl": "openhands",        // claude | openhands | pydantic-ai
  "model": "moonshotai/Kimi-K2.6",
  "provider_id": "nebius"           // references a provider by its provider_id
}
```

A **target-agent profile** bundles `(model, provider, agent_reference)` — no agent impl, because
SIA never runs the target as an engine; it generates and improves the code:

```jsonc
// sia/defaults/profiles/kimi-nebius-target.json
{
  "profile_id": "kimi-nebius-target", // stable id (also the value you pass to --target-agent-profile)
  "name": "Kimi K2.6 on Nebius",
  "model": "moonshotai/Kimi-K2.6",
  "provider_id": "nebius",
  "agent_reference": "default"        // "default" = the task package's reference; see below
}
```

Each file carries both a stable `*_id` (used for references and on the CLI — keep it equal to the
filename stem so name lookups resolve) and a friendly `name` for display.

Bundled profiles:

| Profile | role | agent_impl / reference | model | provider |
|---------|------|------------------------|-------|----------|
| `default-meta` | meta | `agent_impl: claude` | `haiku` | `anthropic` |
| `default-target` | target | `agent_reference: default` | `claude-haiku-4-5-20251001` | `anthropic` |
| `gemini-meta` | meta | `agent_impl: openhands` | `gemini/gemini-3.1-pro-preview` | `gemini` |
| `kimi-nebius-meta` | meta | `agent_impl: openhands` | `moonshotai/Kimi-K2.6` | `nebius` |
| `kimi-nebius-target` | target | `agent_reference: default` | `moonshotai/Kimi-K2.6` | `nebius` |
| `gptoss-tinker-target` | target | `agent_reference: default` | `openai/gpt-oss-120b` | `tinker` |
| `qwen3-tinker-target` | target | `agent_reference: default` | `Qwen/Qwen3-4B-Instruct-2507` | `tinker` |

### agent_reference — the target agent's seed code + deps

A target-agent profile's `agent_reference` is the improvable seed the meta-agent starts from and the
feedback-agent rewrites each generation:

- `"default"` — the task package's bundled `reference/` directory (entrypoint
  `reference_target_agent.py`). This is the historical behavior.
- `{ "source": "./my_agent.py" }` — a single user file; its text is embedded in the meta prompt.
- `{ "source": "./my_agent_dir/", "entrypoint": "main.py" }` — a multi-file directory copied into
  each generation's working dir; the agent reads it with its own tools rather than via the prompt.

Dependencies live in a `requirements.txt` **inside the reference** (not a profile field), installed
per generation on top of the baseline packages — so the meta/feedback agents can evolve them.

### Resolution — name or path

A profile/provider value that contains `/` or ends in `.json` is loaded as a **file path**.
Otherwise a bare **name** resolves in order:

1. the user directory — `$SIA_PROFILES_DIR` / `$SIA_PROVIDERS_DIR`, else `./profiles` / `./providers`;
2. the bundled defaults shipped in the package.

Add your own by dropping a JSON file in `./providers/` or `./profiles/` (no code change):

```bash
sia run --task gpqa --target-agent-profile kimi-nebius-target   # bundled name
sia run --task gpqa --target-agent-profile ./profiles/mine.json # explicit path
```

## Running

### Default (Claude target, Claude meta)

```bash
sia run --task gpqa --max_gen 5 --run_id 1
```

Claude model shortcuts (used by the `claude` agent impl and `claude-*` target models):
`haiku` → `claude-haiku-4-5-20251001`, `sonnet` → `claude-sonnet-4-5-20250929`,
`opus` → `claude-opus-4-5-20251101`.

### Kimi-K2.6 on Nebius as the target model

```bash
export NEBIUS_API_KEY="..."        # target provider
export ANTHROPIC_API_KEY="..."     # default-meta agent
sia run --task gpqa --target-agent-profile kimi-nebius-target --max_gen 5 --run_id 2
```

The meta-agent refactors the reference agent to call the `openai` SDK at the Nebius
`base_url` with `NEBIUS_API_KEY` (dollar-cost is reported as 0 — per-provider pricing is unknown).

### Pointing the meta/feedback agent at another provider

The `claude` agent impl is Anthropic-only (a profile pairing `agent_impl: claude` with a non-anthropic
provider is rejected at load time). To run the meta agent elsewhere, author a profile with the
`openhands` or `pydantic-ai` agent impl:

```jsonc
// ./profiles/gemini-meta.json
{ "profile_id": "gemini-meta", "name": "Gemini meta agent", "agent_impl": "openhands",
  "model": "gemini/gemini-3.1-pro-preview", "provider_id": "gemini" }
```

```bash
sia run --task gpqa --meta-agent-profile gemini-meta
```

Agent-impl model-spec conventions: OpenHands uses fully-qualified `provider/model`
(`gemini/gemini-3.1-pro-preview`, `openai/gpt-4`); PydanticAI uses native specs
(`openai:gpt-4o`, `anthropic:claude-sonnet-4-5-20250929`, `google-gla:gemini-3.1-pro-preview`).
Install the PydanticAI extra with `pip install 'sia-agent[pydantic-ai]'`.

## API keys

Set the `api_key_env` for each provider you use (the orchestrator warns at startup if one is unset):

```bash
export ANTHROPIC_API_KEY="..."   # anthropic provider (claude agent impl / claude target models)
export GEMINI_API_KEY="..."      # gemini provider  (or GOOGLE_API_KEY via openhands)
export OPENAI_API_KEY="..."      # openai provider
export TOGETHER_API_KEY="..."    # together provider
export NEBIUS_API_KEY="..."      # nebius provider
export TINKER_API_KEY="..."      # tinker provider
```

## Comparing multiple LLMs on the same task

```bash
sia run --task gpqa --max_gen 3 --run_id 1 --target-agent-profile default-target      # Claude
sia run --task gpqa --max_gen 3 --run_id 2 --target-agent-profile kimi-nebius-target  # Kimi on Nebius
```

Each run lands in its own `runs/run_{id}/` directory, so they can be compared side by side.

### Choosing where runs are written

By default `sia run` writes (and the auto-started dashboard serves) `./runs`. To
redirect both to another directory, pass `--runs-dir` or set `SIA_RUNS_DIR`
(the flag wins over the env var, which wins over the `./runs` default):

```bash
sia run --task gpqa --runs-dir /data/sia-runs      # flag
SIA_RUNS_DIR=/data/sia-runs sia run --task gpqa    # env var
```

The background dashboard serves exactly the directory the run writes to, so it
always reflects the active run. Point a standalone `sia web` at the same place
with `sia web --runs-dir /data/sia-runs`.

## Visualizing runs

`sia web` serves a dashboard over the `runs/` directory: per-generation
target-agent code (syntax-highlighted), meta/feedback prompts, improvement
plans, evaluation scores (accuracy-across-generations chart + per-domain
breakdown), execution trajectories, and logs.

```bash
sia web                                  # serve ./runs at http://127.0.0.1:8000
sia web --runs-dir ./runs --port 8080    # custom directory / port
```

The same dashboard auto-starts in a background thread during `sia run` so you can
watch generations land live; pass `--no-web` to disable it, or `--web-port` /
`--web-host` to change where it binds. If the `web` extra isn't installed, the
run logs a warning and continues without the dashboard.

## Environment-variable defaults

`SIA_META_PROFILE` / `SIA_TARGET_PROFILE` set the default profile names (overridden by the CLI
flags). `SIA_MAX_GENERATIONS`, `SIA_MAX_TURNS`, and `SIA_SANDBOX_MODE` are also honored.
`SIA_RUNS_DIR` sets the default run-output directory for `sia run` (overridden by `--runs-dir`).

## Notes

- The `claude` agent impl only accepts the Claude shortcut names (`haiku`, `sonnet`, `opus`) and an
  `anthropic` provider. For any other provider, use an `openhands` or `pydantic-ai` profile.
- Make sure the API key matching each chosen provider is in the environment before launching.
