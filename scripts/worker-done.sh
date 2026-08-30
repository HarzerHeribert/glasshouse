#!/usr/bin/env bash
# A worker says, in its own words, that it has finished.
#
# WHY THIS EXISTS
# ---------------
# `worker-watch.sh` decides a worker is finished by reading its cmux pane and
# matching a pattern. On 2026-08-27 that pattern was wrong three times in one
# day: a retry countdown drew no spinner; a spinner glyph was not in the list;
# and then the glyphs that were added turned out to be the SAME glyphs the
# harness prints in its *completion* line, so two finished workers sat
# unreported for 35 and 47 minutes while the watch insisted they were busy.
#
# Each fix was a better guess about a user interface that was never meant to be
# parsed, and cmux exposes no activity state to ask instead. So stop guessing:
# the worker knows when it is done, and this is how it says so.
#
# The watch treats this file as authoritative and keeps the pane heuristic only
# as a FALLBACK, for a worker that crashed, was killed, or forgot — and it says
# which of the two fired, because "the worker signalled" and "the pane went
# quiet" mean different things and only one of them means the work is finished.
#
# USAGE — the last thing a worker does, after writing its report:
#   scripts/worker-done.sh <name> [one-line status]
set -uo pipefail

NAME="${1:?usage: worker-done.sh <name> [status]}"
shift || true
STATUS="${*:-finished}"

# .agent-runtime/done is a single project-wide channel worker-watch.sh reads
# from the main checkout. scripts/ is tracked, so every worktree carries its
# own copy of this script -- and THIS is the last thing a worker does, run
# from the worker's own worktree by design, so BASH_SOURCE alone silently
# forks the signal into that worktree's own throwaway .agent-runtime.
# Reproduced 2026-08-30 (script-tree-audit): invoked the documented way (a
# relative path, from the worker's own worktree cwd), this printed "the
# orchestrator's watch will pick this up within 20s" and wrote the done file
# where no orchestrator watch (rooted in the main checkout) will ever look --
# so the worker's authoritative "I am done" signal never arrives, and
# worker-watch.sh silently falls back to guessing from the pane, every time.
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
DONE_DIR="$REPO/.agent-runtime/done"
mkdir -p "$DONE_DIR" || { echo "worker-done.sh: cannot create $DONE_DIR" >&2; exit 1; }

if ! printf '%s\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$STATUS" > "$DONE_DIR/$NAME"; then
  echo "worker-done.sh: could not write $DONE_DIR/$NAME — the orchestrator was NOT told" >&2
  exit 1
fi
echo "worker-done.sh: '$NAME' signalled done — the orchestrator's watch will pick this up within 20s"
