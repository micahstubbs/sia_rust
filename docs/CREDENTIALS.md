# Credentials setup for sia_rust

To drive a real self-improvement loop you need to build with `--features llm`
and supply the API key(s) for whichever provider(s) your profiles use. This
page lists which environment variable each bundled provider reads and shows two
ways to supply them.

> Keys are only needed at run time, and only for the providers you actually use.
> The default meta/feedback agent talks to Anthropic, so `ANTHROPIC_API_KEY` is
> the most common one to set.

---

## 1. Which variable each provider needs

Each bundled provider declares an `api_key_env` (in `sia/defaults/providers/*.json`).
The native runners read the key from that environment variable.

| Provider (`provider_id`) | Env var | Client kind | Notes |
|---|---|---|---|
| `anthropic` | `ANTHROPIC_API_KEY` | anthropic | Default meta/feedback agent + the `claude` runner. |
| `openai` | `OPENAI_API_KEY` | openai | Also the **fallback** when no provider is supplied to the `openhands` / `pydantic-ai` runners. |
| `nebius` | `NEBIUS_API_KEY` | openai | Nebius Token Factory (OpenAI-compatible). See [NEBIUS_QUICKSTART.md](NEBIUS_QUICKSTART.md). |
| `together` | `TOGETHER_API_KEY` | openai | Together AI (OpenAI-compatible). |
| `gemini` | `GEMINI_API_KEY` | google | Google Gemini (OpenAI-compatible endpoint). |

### Base-URL overrides

- **`ANTHROPIC_BASE_URL`** — optional. When the `claude` runner is used **without
  an explicit provider**, it authenticates via `ANTHROPIC_API_KEY` and honors
  `ANTHROPIC_BASE_URL` as a base-URL override (e.g. a gateway/proxy). If unset,
  it uses the default Anthropic endpoint.
- For OpenAI-compatible providers (`nebius`, `together`, `openai`, custom), the
  base URL comes from the **provider JSON's `base_url`**, not an environment
  variable. To point at a different region/endpoint, create a custom provider
  JSON (see [NEBIUS_QUICKSTART.md](NEBIUS_QUICKSTART.md) §5).

A custom provider can declare any `api_key_env` name it likes; set that
variable the same way you would the bundled ones.

### Web search (optional)

| Capability | Env var | Notes |
|---|---|---|
| Tavily web search | `TAVILY_API_KEY` | Agent-ready web search for the Feedback Agent / legal benchmark task (issue #106). |

[Tavily](https://docs.tavily.com/) returns clean, LLM-native search results. The
**free tier** grants 1,000 credits/month (no credit card to sign up): create a
key at <https://app.tavily.com>, then:

```bash
export TAVILY_API_KEY="tvly-your-tavily-api-key-here"
```

This key is only needed at run time and only when web search is used. The Rust
client lives behind the `llm` feature (`sia::llm::tavily`); it is currently a
*primitive* (serde types + `TavilyClient`) and is not yet wired into the agent
tool loop — that wiring is a follow-up.

---

## 2. Two ways to supply the keys

### Option A — export in your shell

```bash
export ANTHROPIC_API_KEY="your-anthropic-api-key-here"
export NEBIUS_API_KEY="your-nebius-api-key-here"

cargo run --features llm -- run --task gpqa --max_gen 3 --run_id 1
```

### Option B — a `.env` file (loaded at startup)

The `sia` binary loads a `.env` file from the current working directory at
startup (before anything reads credentials). Copy the tracked example and fill
in real keys:

```bash
cp .env.example .env
$EDITOR .env
```

`.env` format (one `KEY=VALUE` per line):

```dotenv
# Comments and blank lines are ignored.
ANTHROPIC_API_KEY=your-anthropic-api-key-here
export NEBIUS_API_KEY=your-nebius-api-key-here   # an `export ` prefix is allowed
OPENAI_API_KEY="quotes are optional"             # surrounding quotes are stripped
```

Notes / precedence:

- **The real environment always wins.** A variable already exported in your
  shell (or set by CI) is *not* overridden by `.env`; `.env` only fills gaps.
- Loading is a **no-op** when no `.env` exists, and it never panics.
- Quoting is minimal: single/double quotes around a value are stripped, but
  there is **no** escape-sequence expansion (`"a\nb"` stays the literal
  characters `a\nb`). Keep values simple.
- **`.env` is gitignored** (the `Environments` section of `.gitignore`) — never
  commit real keys. Only the placeholder `.env.example` is tracked.

When at least one variable is loaded, the binary prints a terse
`loaded N vars from .env` line to stderr.

---

## 3. Verifying

If a required key is missing, the run fails with a clear message naming the
variable, e.g.:

```
provider 'nebius' requires the API key in environment variable 'NEBIUS_API_KEY', which is not set
```

Fix it by exporting the variable or adding it to `.env`, then re-run.
