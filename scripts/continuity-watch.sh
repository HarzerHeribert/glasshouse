#!/usr/bin/env bash
# The continuity thresholds every long-running agent in this project needs,
# in one script that works from any worktree and finds its own session.
#
# WHY THIS IS IN scripts/ AND TRACKED
# -----------------------------------
# Its predecessor lives in `.agent-runtime/`, which `.gitignore` excludes on
# purpose. That made the mechanism invisible to Git: commit 01843eb's message
# describes putting the arming instruction into the relaunch prompt, and the
# commit itself contains a one-line edit to `docs/process/ORIENT.md` and
# nothing else. The change was real and the commit was honest, but the
# repository does not contain it — so it cannot be reviewed, cannot be
# restored from history, and does not survive a fresh clone.
#
# A safety net nobody can review is one nobody can repair.
#
# THE THREE FAILURES THIS FIXES, ALL MEASURED ON 2026-08-29
# ---------------------------------------------------------
#  1. THE PATH DID NOT RESOLVE. Every document says to arm the watch with the
#     relative path `.agent-runtime/continuity-watch.sh`. `.agent-runtime/`
#     exists only in the main checkout, and every worker runs in a worktree —
#     so the documented command resolved in 1 of 64 worktrees and failed with
#     exit 127 in the other 63. Verified by running it: `Monitor` reported the
#     failure as a task notification, which is easy to read as noise, so the
#     session looks armed and is not.
#     This script resolves the repository from its own location instead.
#
#  2. WORKERS WERE NEVER TOLD TO ARM ANYTHING. The instruction was added to the
#     orchestrator's *relaunch* prompt only, so it reached a session only if a
#     previous session already had a watch — the fix could not bootstrap
#     itself, and `scripts/dev/new-worker.sh` never carried it at all. A
#     multi-hour Red-tier worker ran to 33% context with nothing watching it
#     and nothing that would have noticed had it run to 100%.
#     `scripts/tests/test_launch_prompts.py` now fails the gate if any launch
#     prompt drops the instruction.
#
#  3. THE SESSION ID WAS A HAND-COPIED ARGUMENT. The documented recipe is "the
#     last path component of your scratchpad directory". Get it wrong and the
#     watch reads a *different* session's statusline: it is not blind, it is
#     confidently watching someone else, and it never says so. This script
#     discovers its own session by matching the checked-out branch and only
#     falls back to an argument.
#
# BLIND IS NOT SAFE. An unreadable statusline file means the thresholds cannot
# be checked at all; it must say so out loud rather than look like "all clear",
# and it must keep watching rather than exit (practice §54).
#
# THRESHOLDS (the two rate limits are NOT the same, and the difference is the
# user's own correction, recorded in CONTINUATION.md):
#   CTX_PCT >= 75   hand off: a fresh session does the next package cheaper
#   RL5     >= 90   the five-hour window is nearly spent
#   RL7     >= 100  the WEEKLY window is spent. Fires at 100, not 90 —
#                   standing instruction is "continue until depleted", so
#                   interrupting at 90 would stop work the user wants done.
#
# ROLE decides what the CONTEXT threshold tells you to DO, and getting this
# wrong is worse than not firing. The orchestrator hands off by relaunching
# itself. A worker must NOT: `worker-capabilities.md` reserves integrating,
# committing and updating project-status records to the Opus orchestrator, so a
# worker that ran `self-continue.sh` would spawn a second orchestrator into a
# tree that already has one. A worker finishes its report and stops.
#
# USAGE
#   scripts/continuity-watch.sh --role worker
#   scripts/continuity-watch.sh --role orchestrator --session <id> --poll 120
#
#   Monitor(command: "<repo>/scripts/continuity-watch.sh --role worker",
#           persistent: true)
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ROLE=""
SESSID="${CCSL_SESSID:-}"
POLL=120

while [ $# -gt 0 ]; do
  case "$1" in
    --role)    ROLE="${2:?--role needs worker|orchestrator}"; shift 2 ;;
    --session) SESSID="${2:?--session needs an id}"; shift 2 ;;
    --poll)    POLL="${2:?--poll needs seconds}"; shift 2 ;;
    *) echo "continuity-watch: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

case "$ROLE" in
  worker|orchestrator) ;;
  # Not defaulted. The two roles give opposite instructions at the context
  # threshold, and guessing would be worse than refusing.
  *) echo "continuity-watch: --role must be 'worker' or 'orchestrator'" >&2; exit 2 ;;
esac

TMP="${TMPDIR:-/tmp}"

# Find this session's statusline file by the branch it has checked out.
#
# Every agent in this project works in its own worktree on its own branch
# (CLAUDE.md), so the branch identifies the session where a hand-copied id
# only hoped to. Newest match wins if two sessions somehow share a branch, and
# the choice is announced rather than assumed.
discover_sessid() {
  local branch newest=""
  branch="$(git -C "$PWD" rev-parse --abbrev-ref HEAD 2>/dev/null)" || return 1
  [ -n "$branch" ] || return 1
  for f in "$TMP"/ccsl-data-*; do
    [ -r "$f" ] || continue
    grep -qx "BRANCH=${branch}" "$f" 2>/dev/null || continue
    if [ -z "$newest" ] || [ "$f" -nt "$newest" ]; then newest="$f"; fi
  done
  [ -n "$newest" ] || return 1
  grep -m1 '^SESSID=' "$newest" | cut -d= -f2 | tr -d "'\""
}

if [ -z "$SESSID" ]; then
  SESSID="$(discover_sessid || true)"
  if [ -z "$SESSID" ]; then
    # Refusing loudly beats watching nothing quietly. Monitor surfaces stdout.
    echo "CONTINUITY NOT ARMED: could not identify this session from branch '$(git rev-parse --abbrev-ref HEAD 2>/dev/null)'. Pass --session <id> (the last path component of your scratchpad directory)."
    exit 2
  fi
  echo "continuity-watch: watching session ${SESSID} (matched by branch), role ${ROLE}, every ${POLL}s"
fi

DATA="$TMP/ccsl-data-${SESSID}"

# Floats, empty strings and stray quotes all arrive here. Reduce to a bounded
# integer or the string "blind"; never let a malformed value read as zero.
num() {
  local v
  v="$(grep -m1 "^${1}=" "$DATA" 2>/dev/null | cut -d= -f2 | tr -d "'\"" | cut -d. -f1 | tr -dc '0-9')"
  case "$v" in
    '') echo blind; return;;
  esac
  [ "$v" -le 100 ] 2>/dev/null || { echo blind; return; }
  echo "$v"
}

# The relaunch recipe MUST carry the session id, and this is why.
#
# `self-continue.sh` scopes its fire-once lock as `.relaunch-<sessid>-<mode>`,
# and reads that id from `CCSL_SESSID`. That variable is NOT exported into this
# environment, so a recipe that omits it collapses every session's lock to the
# single shared file `.relaunch-unknown-context.lock` -- which is precisely the
# shared-lock defect self-continue.sh's own header says was fixed on 2026-08-26.
# Measured 2026-08-30: a handoff reported "already relaunched; nothing to do"
# against a lock written by an entirely different session, and the orchestrator
# had to work the variable out by hand.
#
# This watch has already discovered or been given the id. Passing it on costs
# nothing, and it is the only place that knows it at the moment the advice is
# printed. A fire-once safety net whose identity degrades to a constant is a
# fire-once-EVER safety net.
if [ "$ROLE" = orchestrator ]; then
  CTX_ACTION="write .agent-runtime/CONTINUATION.md and hand off: CCSL_SESSID=${SESSID} .agent-runtime/self-continue.sh context"
  RL5_ACTION="Checkpoint now; CCSL_SESSID=${SESSID} .agent-runtime/self-continue.sh ratelimit waits for the reset."
else
  CTX_ACTION="finish and WRITE YOUR REPORT NOW, then stop. Do not start new work, and do not run self-continue.sh — that relaunches an orchestrator, which is not your role."
  RL5_ACTION="Write your report with what you have; say in it what is unfinished."
fi

fired_ctx=0; fired_rl5=0; fired_rl7=0; blind_said=0

while true; do
  sleep "$POLL"

  if [ ! -r "$DATA" ]; then
    [ "$blind_said" -eq 0 ] && echo "CONTINUITY BLIND: cannot read $DATA — context and rate-limit thresholds are unchecked. I am still watching."
    blind_said=1
    continue
  fi

  ctx="$(num CTX_PCT)"; rl5="$(num RL5)"; rl7="$(num RL7)"

  if [ "$ctx" = blind ] && [ "$rl5" = blind ] && [ "$rl7" = blind ]; then
    [ "$blind_said" -eq 0 ] && echo "CONTINUITY BLIND: $DATA is readable but holds no usable CTX_PCT/RL5/RL7. Thresholds unchecked."
    blind_said=1
    continue
  fi
  blind_said=0

  if [ "$ctx" != blind ] && [ "$ctx" -ge 75 ] && [ "$fired_ctx" -eq 0 ]; then
    echo "CONTEXT ${ctx}% — ${CTX_ACTION}"
    fired_ctx=1
  fi
  if [ "$rl5" != blind ] && [ "$rl5" -ge 90 ] && [ "$fired_rl5" -eq 0 ]; then
    echo "RL5 ${rl5}% — the five-hour window is nearly spent. ${RL5_ACTION}"
    fired_rl5=1
  fi
  # Deliberately 100, not 90. See the header.
  if [ "$rl7" != blind ] && [ "$rl7" -ge 100 ] && [ "$fired_rl7" -eq 0 ]; then
    echo "RL7 ${rl7}% — the WEEKLY window is depleted. This is the one worth interrupting for. Write the checkpoint."
    fired_rl7=1
  fi
done
