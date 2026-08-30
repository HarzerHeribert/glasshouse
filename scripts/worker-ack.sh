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

# .agent-runtime/{idle,done,dispatched} are single project-wide channels the
# orchestrator's watches read from the main checkout. scripts/ is tracked, so
# every worktree carries its own copy of this script -- deriving REPO from
# BASH_SOURCE alone silently forks these channels per worktree depending on
# invocation form (the same shape reproduced 2026-08-30 in
# scripts/ask-user.sh and scripts/worker-done.sh; this script's own second,
# independent BASH_SOURCE re-derivation below carried the identical bug).
# git's own worktree metadata names the one real answer.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAIN_COMMON="$(git -C "$SCRIPT_DIR" rev-parse --git-common-dir 2>/dev/null)"
case "$MAIN_COMMON" in
  /*) : ;;
  *)  MAIN_COMMON="$(cd "$SCRIPT_DIR/$MAIN_COMMON" 2>/dev/null && pwd -P)" ;;
esac
if [ -n "$MAIN_COMMON" ] && [ "$(basename "$MAIN_COMMON")" = ".git" ]; then
  REPO="$(dirname "$MAIN_COMMON")"
else
  REPO="$SCRIPT_DIR"
fi
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
# Reuse the REPO already resolved above -- re-deriving it from BASH_SOURCE
# here (as this line used to) carried the same wrong-tree bug independently.
if [ -n "${1:-}" ] && [ "${1:-}" != "--list" ]; then
  rm -f "$REPO/.agent-runtime/dispatched/$1"
fi
