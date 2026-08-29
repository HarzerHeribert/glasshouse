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
# The worker's own done signal is cleared here too. Leaving it would make the
# next watch armed for the same name fire instantly on a stale file, which is
# how a "finished" notification arrives for work that has not started.
DONE_FILE="$REPO/.agent-runtime/done/$NAME"

if [ -f "$MARKER" ] || [ -f "$DONE_FILE" ]; then
  rm -f "$MARKER" "$DONE_FILE"
  echo "acknowledged: $NAME"
else
  echo "nothing pending for '$NAME' (already acknowledged, or never reported done)"
fi

# A dispatch marker (written by dev/new-worker.sh so pipeline.sh can see a
# worktree-less recon) stops meaning "live" once the worker is acknowledged.
if [ -n "${1:-}" ] && [ "${1:-}" != "--list" ]; then
  rm -f "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.agent-runtime/dispatched/$1"
fi
