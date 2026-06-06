# Nebius Token Factory quickstart for sia_rust

Nebius Token Factory gives you GPU credits and a library of hosted open-source
models behind an OpenAI-compatible endpoint.  This guide shows you how to get
credentials, point sia_rust at a bundled Nebius profile, and read per-generation
token telemetry — with nothing invented: every flag, profile id, and model slug
below was read directly from the repository files.

---

## 1. What you get

Nebius Token Factory provides:

- **GPU credits** you can spend on API calls.
- **Hosted open-source models** via a single OpenAI-compatible endpoint
  (`https://api.tokenfactory.us-central1.nebius.com/v1/`).

sia_rust ships bundled profiles for four Nebius-hosted models:

| profile_id | model |
|---|---|
| `kimi-nebius-target` | `moonshotai/Kimi-K2.6` |
| `kimi-nebius-meta` | `moonshotai/Kimi-K2.6` |
| `qwen-nebius-target` | `Qwen/Qwen3-Next-80B-A3B-Thinking-fast` |
| `gptoss-nebius-target` | `openai/gpt-oss-120b-fast` |
| `deepseek-nebius-target` | `deepseek-ai/DeepSeek-R1-0528` |

These are fast, demo-friendly choices — no custom provider JSON required to try them.

---

## 2. Get credentials

1. Visit the Nebius Token Factory console and sign up or sign in.
   General documentation: <https://docs.tokenfactory.nebius.com/>
2. Create an API key (sometimes called a service-account token) from the
   Token Factory console.
3. Export the key in your shell:

```bash
export NEBIUS_API_KEY="your-nebius-api-key-here"
```

That is the only credential required to use any of the bundled Nebius profiles.
If you also want the default meta-agent (which uses Claude/Anthropic), add:

```bash
export ANTHROPIC_API_KEY="your-anthropic-api-key-here"
```

---

## 3. Run sia_rust against Nebius

### 3a. Build

The native LLM runners live behind the optional `llm` cargo feature:

```bash
cargo build --features llm
```

### 3b. Run — concrete end-to-end example

The example below runs the `gpqa` task for 3 generations, using:

- **target agent**: `kimi-nebius-target` (Kimi K2.6 on Nebius)
- **meta/feedback agent**: `default-meta` (Claude Haiku on Anthropic — the
  default; set `ANTHROPIC_API_KEY`)

```bash
export NEBIUS_API_KEY="your-nebius-api-key-here"
export ANTHROPIC_API_KEY="your-anthropic-api-key-here"

cargo run --features llm -- run \
  --task gpqa \
  --target-agent-profile kimi-nebius-target \
  --max_gen 3 \
  --run_id 1
```

To use Kimi as both the meta **and** target agent (all traffic goes to Nebius):

```bash
export NEBIUS_API_KEY="your-nebius-api-key-here"

cargo run --features llm -- run \
  --task gpqa \
  --meta-agent-profile kimi-nebius-meta \
  --target-agent-profile kimi-nebius-target \
  --max_gen 3 \
  --run_id 2
```

While the run is live, `sia web` starts automatically at
`http://127.0.0.1:8000` so you can watch generations land; pass `--no-web` to
disable it.

---

## 4. Recommended models — bundled Nebius profiles

All five bundled profiles point at `provider_id: nebius`, which resolves to
`base_url: https://api.tokenfactory.us-central1.nebius.com/v1/`.

| profile_id | name | model | role | agent_impl / reference |
|---|---|---|---|---|
| `kimi-nebius-target` | Kimi K2.6 on Nebius | `moonshotai/Kimi-K2.6` | target | `agent_reference: default` |
| `kimi-nebius-meta` | Kimi K2.6 on Nebius | `moonshotai/Kimi-K2.6` | meta | `agent_impl: openhands` |
| `qwen-nebius-target` | Qwen 80B on Nebius | `Qwen/Qwen3-Next-80B-A3B-Thinking-fast` | target | `agent_reference: default` |
| `gptoss-nebius-target` | GPT OSS 120B on Nebius | `openai/gpt-oss-120b-fast` | target | `agent_reference: default` |
| `deepseek-nebius-target` | DeepSeek R1 on Nebius | `deepseek-ai/DeepSeek-R1-0528` | target | `agent_reference: default` |

**Target profiles** (ending in `-target`) are passed to `--target-agent-profile`.
They carry `(model, provider, agent_reference)` — SIA generates and iterates the
target agent code; it never runs the target as an engine itself.

**Meta profiles** (ending in `-meta`) are passed to `--meta-agent-profile`.
They carry `(agent_impl, model, provider)` and are executed natively by SIA's
LLM runner layer.

---

## 5. Custom provider/profile

You can add your own provider or profile without touching Rust code, by dropping
a JSON file in `./providers/` or `./profiles/` (relative to where you run `cargo
run`).  Alternatively, point to any other directory via environment variables:

```bash
export SIA_PROVIDERS_DIR=/path/to/my/providers
export SIA_PROFILES_DIR=/path/to/my/profiles
```

Resolution order for a bare name (e.g. `--target-agent-profile my-profile`):

1. `$SIA_PROFILES_DIR` (or `./profiles/`) — user directory.
2. The bundled defaults shipped with the package.

A value that contains `/` or ends in `.json` is loaded as a file path directly.

### Minimal provider JSON

```jsonc
// ./providers/my-nebius-eu.json
{
  "provider_id": "my-nebius-eu",
  "name": "Nebius EU region",
  "client_kind": "openai",
  "base_url": "https://api.eu-north1.nebius.com/v1/",
  "api_key_env": "NEBIUS_EU_API_KEY"
}
```

Required fields: `provider_id`, `name`, `client_kind` (`anthropic` | `openai` |
`google`), `api_key_env`.  `base_url` is optional for the built-in providers but
must be set for OpenAI-compatible third-party endpoints.

### Minimal target-agent profile JSON

```jsonc
// ./profiles/my-kimi-target.json
{
  "profile_id": "my-kimi-target",
  "name": "My Kimi target",
  "model": "moonshotai/Kimi-K2.6",
  "provider_id": "my-nebius-eu",
  "agent_reference": "default"
}
```

Required fields for a target profile: `profile_id`, `name`, `model`, `provider_id`.
`agent_reference` defaults to `"default"` when omitted.

### Minimal meta-agent profile JSON

```jsonc
// ./profiles/my-kimi-meta.json
{
  "profile_id": "my-kimi-meta",
  "name": "My Kimi meta agent",
  "agent_impl": "openhands",
  "model": "moonshotai/Kimi-K2.6",
  "provider_id": "my-nebius-eu"
}
```

Required extra field for a meta profile: `agent_impl` (`openhands` or
`pydantic-ai` for non-Anthropic providers — the `claude` impl requires an
`anthropic` provider and is rejected at load time otherwise).

### Use them

```bash
export NEBIUS_EU_API_KEY="your-eu-key"
cargo run --features llm -- run \
  --task gpqa \
  --meta-agent-profile ./profiles/my-kimi-meta.json \
  --target-agent-profile ./profiles/my-kimi-target.json
```

---

## 6. Telemetry / cost visibility

Every generation writes a `telemetry.json` artifact next to
`agent_execution.json` in the run directory (e.g.
`runs/run_1/gen_1/telemetry.json`).  It records, per generation and cumulatively:

- `input_tokens` / `output_tokens` — as reported by the provider API.
- `num_api_calls` — number of model turns in the generation.
- `num_tool_calls` — tool invocations issued by the agent.
- `duration_ms` — wall-clock time.

**Dollar costs are intentionally absent.**  SIA records only token counts and
timing; per-provider pricing is unknown and outside the scope of this codebase
(see the note in `src/llm/telemetry.rs`).  Use the Nebius Token Factory console
to correlate token counts with credit spend.

---

## 7. Troubleshooting

### `NEBIUS_API_KEY` not set

If the key is missing, the provider mapping layer emits:

```
provider 'nebius' requires the API key in environment variable 'NEBIUS_API_KEY', which is not set
```

Fix: `export NEBIUS_API_KEY="..."` before running.

### Rate limits / 429 errors

sia_rust's retry layer (exponential backoff) retries transient errors
automatically.  If you consistently hit rate limits, reduce `--max_gen` or the
number of parallel runs, or check your Token Factory quota in the console.

### Wrong region / `base_url` mismatch

The bundled Nebius provider always uses
`https://api.tokenfactory.us-central1.nebius.com/v1/`.  If you need a different
region, create a custom provider JSON (see Section 5) with the correct
`base_url` and a matching `api_key_env`.

### Model not found

If a request fails with a model-not-found error, verify the exact model slug in
the Nebius Token Factory console — model identifiers are case-sensitive and
version-stamped (e.g. `deepseek-ai/DeepSeek-R1-0528`, not `DeepSeek-R1`).
The bundled profiles use the slugs current at the time they were added; update
your custom profile JSON if a model is renamed or versioned.

### `claude` agent_impl with a Nebius provider

The `claude` agent impl requires an `anthropic` provider and is rejected at load
time with a clear error if paired with `nebius`.  Use `agent_impl: openhands` or
`agent_impl: pydantic-ai` for Nebius meta-agent profiles (as `kimi-nebius-meta`
does).
