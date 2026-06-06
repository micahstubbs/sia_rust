#!/usr/bin/env bash
#
# verify_nebius_models.sh — list (and optionally check) the model slugs that are
# live on Nebius Token Factory's OpenAI-compatible /v1/models catalog.
#
# Why: sia_rust ships bundled Nebius profiles whose `model` slugs are
# version-stamped and can be renamed/retired by the provider. Before relying on
# a profile (e.g. before a hackathon run) confirm its slug still exists in the
# live catalog. This script does exactly that, using only `curl` + `jq`.
#
# Usage:
#   export NEBIUS_API_KEY="your-key"
#   scripts/verify_nebius_models.sh                 # print all model ids, sorted
#   scripts/verify_nebius_models.sh <model-slug>    # check one slug is present
#
# Environment:
#   NEBIUS_API_KEY   (required) your Token Factory API key / service-account token
#   NEBIUS_BASE_URL  (optional) override the catalog base URL; defaults to the
#                    us-central1 region used by the bundled `nebius` provider.
#                    Must NOT include a trailing slash (we append "/models").
#
# No-jq fallback (one-liner, prints raw JSON ids via python):
#   curl -s "$NEBIUS_BASE_URL/models" -H "Authorization: Bearer $NEBIUS_API_KEY" \
#     | python3 -c 'import json,sys; [print(m["id"]) for m in sorted(json.load(sys.stdin)["data"], key=lambda m: m["id"])]'

set -euo pipefail

# Default to the same region/base URL the bundled `nebius` provider uses
# (sia/defaults/providers/nebius.json), minus the trailing slash.
BASE_URL="${NEBIUS_BASE_URL:-https://api.tokenfactory.us-central1.nebius.com/v1}"

# --- preflight: require the API key -----------------------------------------
if [[ -z "${NEBIUS_API_KEY:-}" ]]; then
  echo "error: NEBIUS_API_KEY is not set." >&2
  echo "       export NEBIUS_API_KEY=\"your-nebius-api-key\" and re-run." >&2
  echo "       Get a key from the Nebius Token Factory console:" >&2
  echo "       https://docs.tokenfactory.nebius.com/" >&2
  exit 1
fi

# --- preflight: require a JSON tool ------------------------------------------
if command -v jq >/dev/null 2>&1; then
  JSON_TOOL="jq"
elif command -v python3 >/dev/null 2>&1; then
  JSON_TOOL="python3"
else
  echo "error: neither 'jq' nor 'python3' found; one is required to parse JSON." >&2
  echo "       Install jq (https://jqlang.github.io/jq/) or use the no-jq" >&2
  echo "       one-liner documented in docs/NEBIUS_MODELS.md." >&2
  exit 1
fi

# --- fetch the catalog -------------------------------------------------------
# The endpoint is OpenAI-compatible: GET {base}/models -> { "data": [ {"id": ...} ] }
RESPONSE="$(curl -fsS "${BASE_URL}/models" -H "Authorization: Bearer ${NEBIUS_API_KEY}")" || {
  echo "error: request to ${BASE_URL}/models failed." >&2
  echo "       Check NEBIUS_API_KEY, NEBIUS_BASE_URL (region), and connectivity." >&2
  exit 1
}

# Extract a sorted list of model ids using whichever JSON tool is available.
if [[ "$JSON_TOOL" == "jq" ]]; then
  MODEL_IDS="$(printf '%s' "$RESPONSE" | jq -r '.data[].id' | sort)"
else
  MODEL_IDS="$(printf '%s' "$RESPONSE" | python3 -c \
    'import json,sys; print("\n".join(sorted(m["id"] for m in json.load(sys.stdin)["data"])))')"
fi

# --- mode 1: check a single slug --------------------------------------------
if [[ $# -ge 1 ]]; then
  WANTED="$1"
  if printf '%s\n' "$MODEL_IDS" | grep -Fxq -- "$WANTED"; then
    echo "OK: '${WANTED}' is present in the live Nebius catalog."
    exit 0
  else
    echo "MISSING: '${WANTED}' is NOT in the live Nebius catalog." >&2
    echo "Closest matches:" >&2
    printf '%s\n' "$MODEL_IDS" | grep -Fi -- "${WANTED%%/*}" >&2 || true
    exit 2
  fi
fi

# --- mode 2: list all slugs --------------------------------------------------
printf '%s\n' "$MODEL_IDS"
