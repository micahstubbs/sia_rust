#!/usr/bin/env bash
# Send a PDF as an email attachment via the Resend API.
#
# Usage:
#   RESEND_API_KEY=re_xxx ./scripts/send-paper-email.sh \
#       --from "Micah Stubbs <micah@casemirror.ai>" \
#       --to   "willstark.lab@gmail.com" \
#       --subject "SIA Rust paper (preprint draft)" \
#       --pdf  docs/paper/sia_rust.pdf \
#       --body-file /path/to/body.txt
#
# RESEND_API_KEY must be a Resend key whose account has the FROM domain verified.
set -euo pipefail

FROM="" TO="" SUBJECT="" PDF="" BODY_FILE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --from) FROM="$2"; shift 2;;
    --to) TO="$2"; shift 2;;
    --subject) SUBJECT="$2"; shift 2;;
    --pdf) PDF="$2"; shift 2;;
    --body-file) BODY_FILE="$2"; shift 2;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

: "${RESEND_API_KEY:?set RESEND_API_KEY}"
[[ -n "$FROM" && -n "$TO" && -n "$SUBJECT" && -n "$PDF" ]] || { echo "missing required arg" >&2; exit 2; }
[[ -f "$PDF" ]] || { echo "pdf not found: $PDF" >&2; exit 2; }

BODY="$(cat "${BODY_FILE:-/dev/null}")"
B64="$(base64 -i "$PDF" | tr -d '\n')"
FILENAME="$(basename "$PDF")"

PAYLOAD="$(jq -n \
  --arg from "$FROM" --arg to "$TO" --arg subject "$SUBJECT" \
  --arg text "$BODY" --arg fn "$FILENAME" --arg content "$B64" \
  '{from:$from, to:[$to], subject:$subject, text:$text,
    attachments:[{filename:$fn, content:$content}]}')"

RESP="$(curl -s -w '\n%{http_code}' -X POST https://api.resend.com/emails \
  -H "Authorization: Bearer $RESEND_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD")"

CODE="$(printf '%s' "$RESP" | tail -n1)"
JSON="$(printf '%s' "$RESP" | sed '$d')"
echo "HTTP $CODE"
echo "$JSON"
[[ "$CODE" == 2* ]] || exit 1
