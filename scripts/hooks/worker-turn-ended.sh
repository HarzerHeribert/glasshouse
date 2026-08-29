#!/usr/bin/env bash
# Claude Code `Stop` hook: tell the orchestrator this worker's turn ended.
#
# WHY THIS EXISTS
# ---------------
# `worker-watch.sh` used to decide a worker had finished by matching a pattern
# against its cmux pane. On 2026-08-27 that pattern was wrong three times in one
# day — a retry countdown drew no spinner, a spinner glyph was missing from the
# list, and then the glyphs added to fix that turned out to be the SAME glyphs
# the harness prints in its *completion* line, so two finished workers sat
# unreported for 35 and 47 minutes while the watch insisted they were busy.
#
# Every one of those fixes was a better guess about a user interface that was
# never meant to be parsed. The harness already emits the event; this listens to
# it instead. `docs/process/harness-hook-protocol.md` calls this `turn.completed`
# and is explicit that it is **advisory**: a turn ending means the model stopped,
# not that the work is right. The orchestrator still reviews the report, the
# diff and the gate.
#
# The worker's name is its worktree directory, so nothing per-worker has to be
# configured: `/Users/eneas/projects/glasshouse-windows-session` is
# `windows-session`. The done file is written into the MAIN checkout, which is
# where the watch looks and which every worktree can locate through git.
#
# Deliberately silent and always successful. A hook that fails, blocks, or
# prints to a worker's transcript is a hook that costs more than it reports.
set -uo pipefail

dir="${CLAUDE_PROJECT_DIR:-$PWD}"
name="$(basename "$dir")"

# The orchestrator works in the main checkout and must never signal itself.
#
# TWO LAYOUTS, and missing the second one killed this hook silently.
#
# Worker worktrees used to be siblings named `glasshouse-<name>`. Practice §73
# moved them INSIDE the repo, to `.worktrees/<name>`, and this case statement was
# not updated — so every worker under the new layout fell through to `*) exit 0`
# and NO worker has emitted a done signal since. The symptom was visible in every
# watch alarm ("pane went quiet, NO done signal") and read past for a whole batch,
# because a missing signal looks exactly like a worker that simply had not
# finished. That is the same shape as a check matching nothing and reporting
# PASSED: absence of a signal is not evidence of anything.
parent="$(basename "$(dirname "$dir")")"
case "$name" in
  glasshouse) exit 0 ;;                 # the main checkout: the orchestrator
  glasshouse-*) name="${name#glasshouse-}" ;;   # legacy sibling worktrees
  *)
    # Current layout: <repo>/.worktrees/<name>
    [ "$parent" = ".worktrees" ] || exit 0
    ;;
esac

# `git worktree list` prints the main checkout first, whichever worktree asks.
main="$(git -C "$dir" worktree list 2>/dev/null | head -1 | awk '{print $1}')"
[ -n "$main" ] && [ -d "$main/.agent-runtime" ] || exit 0

mkdir -p "$main/.agent-runtime/done" 2>/dev/null || exit 0
printf '%s\tturn ended (Stop hook)\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  > "$main/.agent-runtime/done/$name" 2>/dev/null || exit 0
exit 0
