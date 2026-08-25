#!/usr/bin/env bash
# Tick a finished worker off the list, which is what stops its watch nagging.
#
#   scripts/worker-ack.sh <name>      acknowledge one worker
#   scripts/worker-ack.sh --list      show everything still waiting
#
# Acknowledge only after you have ACTUALLY DEALT WITH IT: read the report,
# inspected the diff, run the gates yourself. Ticking it off to silence the
# reminder is the exact failure the reminder exists to prevent.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IDLE_DIR="$REPO/.agent-runtime/idle"
mkdir -p "$IDLE_DIR"

if [ "${1:-}" = "--list" ] || [ $# -eq 0 ]; then
  shopt -s nullglob
  pending=("$IDLE_DIR"/*)
  if [ ${#pending[@]} -eq 0 ]; then
    echo "no workers waiting to be acknowledged"
  else
    echo "waiting to be acknowledged:"
    for f in "${pending[@]}"; do
      name="$(basename "$f")"
      stamped="$(cat "$f" 2>/dev/null || echo 0)"
      mins=$(( ( $(date +%s) - stamped ) / 60 ))
      echo "  - $name (idle, last reminded ${mins}m ago)"
    done
  fi
  exit 0
fi

NAME="$1"
MARKER="$IDLE_DIR/$NAME"
if [ -f "$MARKER" ]; then
  rm -f "$MARKER"
  echo "acknowledged: $NAME"
else
  echo "nothing pending for '$NAME' (already acknowledged, or never went idle)"
fi
