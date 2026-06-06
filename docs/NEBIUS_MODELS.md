# Verifying Nebius model slugs

Nebius Token Factory version-stamps its model slugs (e.g.
`deepseek-ai/DeepSeek-R1-0528`, not `DeepSeek-R1`) and can rename or retire
them over time. sia_rust ships bundled Nebius profiles that each pin an exact
`model` slug, so before a run — especially before a hackathon — it's worth
confirming each slug still exists in the live catalog.

This page covers: how to list the live catalog, how to run the offline-skipped
verification test, the verification status of each bundled slug, recommended
strong models for the meta/feedback role, and what to do when a model is
temporarily unavailable.

> The companion [NEBIUS_QUICKSTART.md](NEBIUS_QUICKSTART.md) covers credentials,
> running, and telemetry. This page is just about model-slug verification.

---

## 1. List the live `/v1/models` catalog

Nebius speaks the OpenAI protocol, so the catalog is a plain
`GET {base_url}/models` returning `{ "data": [ { "id": ... } ] }`.

### With the bundled script (recommended)

```bash
export NEBIUS_API_KEY="your-nebius-api-key"
scripts/verify_nebius_models.sh                  # print all model ids, sorted
scripts/verify_nebius_models.sh openai/gpt-oss-120b-fast   # check ONE slug
```

The script errors helpfully if `NEBIUS_API_KEY` is unset, defaults to the same
`us-central1` base URL as the bundled `nebius` provider, and accepts a
`NEBIUS_BASE_URL` override for other regions. It uses `jq` if present, else
falls back to `python3`.

### One-liner (no jq, just curl + python)

```bash
curl -s "https://api.tokenfactory.us-central1.nebius.com/v1/models" \
  -H "Authorization: Bearer $NEBIUS_API_KEY" \
  | python3 -c 'import json,sys; [print(m["id"]) for m in sorted(json.load(sys.stdin)["data"], key=lambda m: m["id"])]'
```

### One-liner (with jq)

```bash
curl -s "https://api.tokenfactory.us-central1.nebius.com/v1/models" \
  -H "Authorization: Bearer $NEBIUS_API_KEY" | jq -r '.data[].id' | sort
```

---

## 2. Run the offline-skipped verification test

There is an `#[ignore]` integration test (`tests/nebius_live.rs`, gated behind
the `llm` feature) that loads **every bundled Nebius profile** and asserts each
profile's `model` slug appears in the live `/v1/models` catalog. It is ignored
by default so offline CI is unaffected, and it skips cleanly if the key is unset.

```bash
export NEBIUS_API_KEY="your-nebius-api-key"
cargo test --features llm --ignored nebius
```

On failure it prints exactly which `profile_id -> model` slugs are missing,
alongside the full live catalog, so you can pick a replacement slug.

---

## 3. Bundled Nebius profiles — verification status

All bundled profiles point at `provider_id: nebius`
(`base_url: https://api.tokenfactory.us-central1.nebius.com/v1/`). "Verified?"
below reflects **public-doc corroboration only** — the live `/v1/models` call
above (or the test in section 2) is the authoritative check for your account/region.

| profile_id | model slug | role | verified? |
|---|---|---|---|
| `deepseek-nebius-target` | `deepseek-ai/DeepSeek-R1-0528` | target | Yes — appears verbatim in the official Token Factory quickstart code examples ([docs](https://docs.tokenfactory.nebius.com/quickstart)) |
| `qwen-nebius-target` | `Qwen/Qwen3-Next-80B-A3B-Thinking-fast` | target | Web-corroborated — listed in the Nebius model catalog ([Mastra Nebius models](https://mastra.ai/models/providers/nebius)) |
| `gptoss-nebius-target` | `openai/gpt-oss-120b-fast` | target | Web-corroborated — listed in the catalog ([Mastra](https://mastra.ai/models/providers/nebius)) and used in a Rust rig-core Nebius example ([rup12.net](https://rup12.net/posts/adding-support-for-nebius-token-factory-to-rig/)) |
| `kimi-nebius-target` | `moonshotai/Kimi-K2.6` | target | **Needs live check** — public sources show `moonshotai/Kimi-K2.5` / `Kimi-K2.5-fast` / `Kimi-K2-Instruct`; the exact `Kimi-K2.6` slug could not be corroborated. Run section 1/2 to confirm, or switch to a verified Kimi slug. |
| `kimi-nebius-meta` | `moonshotai/Kimi-K2.6` | meta | **Needs live check** — same caveat as `kimi-nebius-target`. |

The Kimi profiles are intentionally left in place (the `K2.6` slug may well be
live on your account) but flagged: confirm with `scripts/verify_nebius_models.sh
moonshotai/Kimi-K2.6` before relying on them, and substitute the live Kimi slug
in a custom profile if it has moved (see
[NEBIUS_QUICKSTART.md §5](NEBIUS_QUICKSTART.md#5-custom-providerprofile)).

---

## 4. Recommended strong models for the meta/feedback role

The meta/feedback agent benefits from a strong reasoning model. The following
are good candidates whose slugs are web-corroborated in the Nebius catalog; add
them as custom profiles (or new bundled profiles) **only after confirming the
exact slug** via section 1/2:

| model | slug (verify before use) | corroboration |
|---|---|---|
| DeepSeek R1 | `deepseek-ai/DeepSeek-R1-0528` | verified (official docs) — already bundled as `deepseek-nebius-target` |
| Qwen3 Next 80B (thinking) | `Qwen/Qwen3-Next-80B-A3B-Thinking-fast` | web-corroborated — already bundled as `qwen-nebius-target` |
| Llama 3.3 70B Instruct | `meta-llama/Llama-3.3-70B-Instruct` | web-corroborated ([Mastra](https://mastra.ai/models/providers/nebius), [OpenRouter](https://openrouter.ai/provider/nebius)) — **not yet bundled**; add after verifying the slug |
| gpt-oss 120B | `openai/gpt-oss-120b-fast` (or `openai/gpt-oss-120b`) | web-corroborated — already bundled as `gptoss-nebius-target` |

A meta profile uses `agent_impl: openhands` (or `pydantic-ai`) for Nebius — the
`claude` impl requires an Anthropic provider. See
[NEBIUS_QUICKSTART.md §5](NEBIUS_QUICKSTART.md#5-custom-providerprofile) for the
minimal meta-profile JSON.

> Llama 3.3 70B is documented here as a "add after verifying the slug" candidate
> rather than shipped as a bundled profile, because we could not confirm it via
> the live `/v1/models` catalog from this environment (no API key available).

---

## 5. Fallback when a model is temporarily unavailable

If a request fails with a model-not-found / unavailable error:

1. **Re-list the catalog** (section 1) to see the current exact slug — it may
   have been version-bumped (e.g. a `-fast` variant added/removed).
2. **Switch profile**: pick another bundled Nebius profile that verifies, e.g.
   `--target-agent-profile deepseek-nebius-target` (R1 is the most reliably
   verified slug).
3. **Switch provider**: any bundled non-Nebius profile (e.g. `default-target`
   on Anthropic) works as a fallback if Nebius is degraded — set the matching
   API key and pass the profile.
4. **Custom slug**: drop a custom profile JSON with the live slug without
   touching Rust (see [NEBIUS_QUICKSTART.md §5](NEBIUS_QUICKSTART.md#5-custom-providerprofile)).

The retry layer (exponential backoff) already handles transient 429/5xx; these
steps are for a slug that is genuinely renamed or retired.

---

## 6. Transport wiring (already in place)

The OpenAI-compatible / rig-core wiring for Nebius is **already implemented** —
this page documents how to verify slugs, not how to wire a client. The
`nebius` provider has `client_kind: openai`, and
[`src/llm/provider_mapping.rs`](../src/llm/provider_mapping.rs) maps any
`openai`-kind provider to an `AgentClient::Chat` over the
`HttpChatTransport` (`POST {base_url}/chat/completions`), resolving the
provider's `base_url` and `NEBIUS_API_KEY` bearer auth. `client_for`,
`chat_transport_for`, and `base_url_for` there are the single source of truth;
the native runners (openhands / pydantic-ai) consume that transport, and
`rig-core` is the underlying OpenAI-compatible client dependency (see
`Cargo.toml`, optional behind the `llm` feature). No additional provider code is
needed to use a verified Nebius slug.
