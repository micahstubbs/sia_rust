#!/usr/bin/env bash
# Mirror all open GitHub issues into the beads tracker so agents can pick them up.
# Idempotency: skips creation if a beads issue already carries the GH external-ref.
# Writes a GH#->beads-id map to scripts/.gh_to_beads.map
set -euo pipefail

REPO="micahstubbs/sia_rust"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAP="$ROOT/scripts/.gh_to_beads.map"
: > "$MAP"

gh issue list --state open --limit 100 \
  --json number,title,body,labels > /tmp/gh_open_issues.json

count=$(jq length /tmp/gh_open_issues.json)
echo "Mirroring $count open GitHub issues into beads..."

# GH numbers already mirrored (by external-ref) -> skip for idempotency
existing=$(br list --json 2>/dev/null \
  | jq -r '.issues[].external_ref // empty' \
  | sed -n 's#.*/issues/\([0-9]*\)$#\1#p' | sort -u)

# label-set membership helper
has() { echo "$1" | tr ',' '\n' | grep -qx "$2"; }

for i in $(seq 0 $((count - 1))); do
  num=$(jq -r ".[$i].number" /tmp/gh_open_issues.json)
  title=$(jq -r ".[$i].title" /tmp/gh_open_issues.json)
  body=$(jq -r ".[$i].body // \"\"" /tmp/gh_open_issues.json)
  labels=$(jq -r "[.[$i].labels[].name] | join(\",\")" /tmp/gh_open_issues.json)
  ref="https://github.com/$REPO/issues/$num"

  if echo "$existing" | grep -qx "$num"; then
    echo "  GH #$num already mirrored — skipping"
    continue
  fi

  # type mapping
  if [ "$num" = "34" ]; then
    type="epic"
  elif has "$labels" "bug"; then
    type="bug"
  else
    type="task"
  fi

  # priority mapping
  if has "$labels" "high-priority" || [ "$num" = "34" ]; then
    prio=1
  elif [ "$num" = "105" ] || [ "$num" = "106" ] || [ "$num" = "107" ] || [ "$num" = "108" ]; then
    prio=3   # sponsor/credit coordination (manual, not code)
  elif has "$labels" "quick-win"; then
    prio=2
  else
    prio=2
  fi

  desc=$(printf '%s\n\n---\nMirrors GitHub #%s — %s' "$body" "$num" "$ref")

  id=$(br create "$title" \
        --type "$type" \
        --priority "$prio" \
        --labels "$labels" \
        --description "$desc" \
        --external-ref "$ref" \
        --json 2>/dev/null | jq -r '.id // .ID // empty')

  if [ -z "$id" ]; then
    echo "  !! failed GH #$num ($title)"
    continue
  fi
  echo "$num=$id" >> "$MAP"
  printf '  GH #%-3s -> %-10s [%s p%s] %s\n' "$num" "$id" "$type" "$prio" "$title"
done

echo "Map written to $MAP"
