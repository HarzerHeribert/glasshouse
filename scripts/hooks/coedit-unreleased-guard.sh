#!/usr/bin/env bash
# Stop hook: refuse to finish while this session still holds a co-edit claim.
#
# WHY THIS EXISTS
# ---------------
# `scripts/coedit.sh` lets two workers share one file (practice §77, map Maybe L)
# by having each CLAIM it, work in its own worktree, read the peer's diff once at
# finalization, and then DECLARE DONE. The barrier opens when every claimant has
# declared, and only then does the orchestrator reconcile and RELEASE.
#
# Nothing enforced either half. A worker that finished its task without running
# `coedit.sh done` left the barrier one short **forever**: the peer waits on a
# signal that will never come, the orchestrator is never told the file is ready,
# and the file stays marked contended for every later round. The same leak at the
# other end: an orchestrator that reconciles and forgets `coedit.sh release`
# leaves a stale claim that makes the next round think a file is shared when it
# is not.
#
# Both failures are silent, and both are indistinguishable from "still working".
# That is the shape this project keeps paying for — a missing signal read as an
# absent problem. So it is a gate now, not a habit.
#
# WHAT IT DOES
#   worker  (running inside <repo>/.worktrees/<name>)
#       blocks if it holds a claim with no matching `done`
#   orchestrator (the main checkout)
#       blocks if a file's barrier is OPEN — every claimant done — but the file
#       was never released, which means a reconciliation was forgotten
#
# It BLOCKS rather than warns, because a warning printed into a transcript that
# is about to end is a warning nobody reads. It blocks at most once per session:
# Claude Code sets `stop_hook_active` when it is re-entering after a block, and
# honouring that is what keeps this from becoming an infinite loop.
#
# Registered on `Stop` in `.claude/settings.json`.
set -uo pipefail

INPUT="$(cat 2>/dev/null || true)"

# Never block twice. Without this the model can be pinned in a stop/block loop,
# which is worse than the leak this prevents.
case "$INPUT" in
  *'"stop_hook_active":true'*|*'"stop_hook_active": true'*) exit 0 ;;
esac

dir="${CLAUDE_PROJECT_DIR:-$PWD}"
name="$(basename "$dir")"
parent="$(basename "$(dirname "$dir")")"

main="$(git -C "$dir" worktree list 2>/dev/null | head -1 | awk '{print $1}')"
[ -n "$main" ] || exit 0
ROOT="$main/.agent-runtime/coedit"
[ -d "$ROOT" ] || exit 0

# The worker id is its worktree directory name, matching how packets tell workers
# to claim (`coedit.sh claim CLAUDE.md tool-mutate` from `.worktrees/tool-mutate`).
role="orchestrator"
case "$name" in
  glasshouse) role="orchestrator" ;;
  glasshouse-*) role="worker"; name="${name#glasshouse-}" ;;
  *) if [ "$parent" = ".worktrees" ]; then role="worker"; else exit 0; fi ;;
esac

# `slug()` in coedit.sh maps a path to a directory name; the reverse is not
# needed because each state dir records the file it belongs to only through its
# own name. Report the slug, and give the exact command either way.
# Emit a Claude Code hook decision as JSON. The reason is arbitrary prose that
# carries real newlines and may carry quotes or backslashes, none of which a
# printf'd string literal can survive: a raw newline inside a JSON string is
# invalid JSON, so the old form failed on EVERY firing, not just on odd names.
emit_block() {
  REASON="$(printf '%b' "$1")" python3 -c 'import json,os,sys; sys.stdout.write(json.dumps({"decision":"block","reason":os.environ["REASON"]})+"\n")'
}

unreleased=""

if [ "$role" = "worker" ]; then
  for d in "$ROOT"/*/; do
    [ -d "$d/claims" ] || continue
    [ -e "$d/claims/$name" ] || continue
    [ -e "$d/done/$name" ] && continue
    unreleased="$unreleased  $(basename "$d")\n"
  done
  if [ -n "$unreleased" ]; then
    emit_block "CO-EDIT CLAIM NOT RELEASED. You claimed a shared file and are ending without declaring done, which leaves the barrier one claimant short forever: your peer waits on a signal that will never come and the orchestrator is never told the file is ready.\n\nStill held by '$name':\n$unreleased\nBefore you finish, for each file above:\n  1. scripts/coedit.sh diff <file> $name   (read the peer ONCE, adapt if needed)\n  2. scripts/coedit.sh done <file> $name\n\nThen say in your report what you changed because of what you saw - 'no adaptation needed' is a real result. If you genuinely did not edit the file, run 'done' anyway: the barrier counts claimants, not edits."
    exit 0
  fi
  exit 0
fi

# Orchestrator: a barrier that opened and was never released.
for d in "$ROOT"/*/; do
  [ -d "$d/claims" ] || continue
  total=0; done_n=0
  for c in "$d/claims"/*; do
    [ -e "$c" ] || continue
    total=$((total+1))
    [ -e "$d/done/$(basename "$c")" ] && done_n=$((done_n+1))
  done
  [ "$total" -gt 0 ] && [ "$done_n" -eq "$total" ] && unreleased="$unreleased  $(basename "$d")\n"
done

if [ -n "$unreleased" ]; then
  emit_block "CO-EDIT BARRIER OPEN AND NOT RELEASED. Every claimant finished on the file(s) below, so reconciliation is yours and was not recorded. Leaving it unreleased makes the next round believe the file is still contended.\n\n$unreleased\nReconcile, then: scripts/coedit.sh release <file>\n\nMerge each worker's own hunks. If both intents cannot be preserved, escalate with both visible - never invent a merge neither worker wrote."
  exit 0
fi
exit 0
