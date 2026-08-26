#!/usr/bin/env bash
# A dead-man's switch on the ORCHESTRATOR, not on its workers.
#
# WHY THIS EXISTS
# ---------------
# Every other watch in this project is event-driven: a worker changes state and
# something fires. That covers the case where work is running. It does not cover
# the case the user actually reported — **an orchestrator sitting idle with
# nothing running and work still left**, because when there is no event there is
# nothing to fire.
#
# An idle orchestrator waiting on agents is fine. An idle orchestrator waiting
# on nothing is the fleet stopped, silently, with the machine still awake.
#
# So this watches the orchestrator's own pane the way worker-watch.sh watches a
# worker's, and emits when all three hold:
#
#   1. the orchestrator's surface has been idle for IDLE_CHECKS in a row;
#   2. no worker is waiting to be acknowledged;
#   3. the capability map still has open boxes.
#
# Drive it with Monitor, persistent, ONE per orchestrator:
#   Monitor(command: "scripts/orchestrator-heartbeat.sh surface:84", persistent: true)
#
# USAGE
#   scripts/orchestrator-heartbeat.sh <own-surface-ref> [poll-seconds] [idle-checks]
#
# STOPPING IT
#   touch .agent-runtime/stopped     # the user asked it to stop; stay quiet
#   rm    .agent-runtime/stopped     # resume nudging
set -uo pipefail

SURFACE="${1:?usage: orchestrator-heartbeat.sh <own-surface-ref> [poll] [idle-checks]}"
POLL="${2:-120}"
IDLE_CHECKS="${3:-4}"          # 4 × 120s ≈ 8 minutes of genuine quiet

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IDLE_DIR="$REPO/.agent-runtime/idle"
STOP_FLAG="$REPO/.agent-runtime/stopped"

# Same busy test worker-watch.sh uses, and for the same reason: read the
# SURFACE, never the workspace — an empty ref resolves to the caller's own pane.
is_busy() {
  local screen
  screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)" || return 1
  printf '%s' "$screen" | grep -qE 'esc to interrupt|esc to cancel|[0-9]+s · ↓|[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]'
}

# Two different things, and conflating them was this script's first defect:
# a worker WAITING for review leaves a marker; a worker still WORKING leaves
# nothing but its watch. The heartbeat fired within minutes of being armed,
# while `round-tools` was mid-batch, because it only asked the first question.
workers_waiting() {
  shopt -s nullglob
  local pending=("$IDLE_DIR"/*)
  [ ${#pending[@]} -gt 0 ]
}

# A live `worker-watch.sh` is a worker that was dispatched and has not been
# acknowledged — which is exactly "still running or still owed a review".
# Match the bash process, not the shell wrapper that spawned it, or every
# watch counts twice.
workers_running() {
  pgrep -f '^(bash|/bin/bash).*worker-watch\.sh' >/dev/null 2>&1
}

MAP="$REPO/docs/product/capability-map.md"

# The orchestrator's own context, as a percentage, or "unknown".
#
# The statusline writes this ~1/s; it is the only shell-readable source, because
# Claude Code pushes the numbers to the statusline rather than to any API. When
# it is absent the heartbeat still nudges — it just cannot advise on cost.
context_pct() {
  local d="${TMPDIR:-/tmp}/ccsl-data-${CCSL_SESSID:-unknown}"
  [ -r "$d" ] || { echo unknown; return; }
  local v
  v="$(sed -n 's/^CTX_PCT=//p' "$d" | head -1)"
  case "$v" in ''|*[!0-9]*) echo unknown;; *) echo "$v";; esac
}

# Returns the open-box count, or the string "blind" when the map cannot be read.
#
# **`|| echo 0` was here and it was a real defect.** When the capability map
# moved to docs/product/, a monitor still holding the old path got a failed grep,
# turned it into zero, and announced "the capability map has no open boxes left.
# Nothing to do" — then exited. There were 1,091. A watchdog that reports success
# when it has gone blind is worse than no watchdog, because it also stops.
#
# Not-readable and none-left are different answers and must stay different.
open_boxes() {
  [ -r "$MAP" ] || { echo blind; return; }
  grep -c '^☐' "$MAP" 2>/dev/null || echo blind
}

quiet=0
nudged=0

while true; do
  sleep "$POLL"

  # The user asked it to stop. Stopping is a decision, not a fault.
  if [ -f "$STOP_FLAG" ]; then quiet=0; nudged=0; continue; fi

  if is_busy; then
    quiet=0
    nudged=0
    continue
  fi

  # Idle — but idle *because workers are busy* is correct and not our business,
  # and so is idle with a report sitting unreviewed (that has its own nagging
  # watch and does not need a second voice).
  if workers_running || workers_waiting; then
    quiet=0
    continue
  fi

  quiet=$((quiet + 1))
  [ "$quiet" -lt "$IDLE_CHECKS" ] && continue

  remaining="$(open_boxes)"
  if [ "$remaining" = "blind" ]; then
    # Loud, and keep going: the map may be mid-move, and a watchdog that quits
    # on a transient read failure is a watchdog that quits.
    echo "HEARTBEAT BLIND: cannot read $MAP — I cannot tell whether work remains. Check the path; I am still watching."
    quiet=0
    sleep 600
    continue
  fi
  if [ "$remaining" -eq 0 ]; then
    echo "ORCHESTRATOR IDLE and the capability map has no open boxes left. Nothing to do."
    exit 0
  fi

  if [ "$nudged" -eq 0 ]; then
    # Waking a large context is expensive, and the nudge that causes it should
    # say so. At 700k tokens the correct move is to hand off to a fresh
    # orchestrator, not to start a package — a successor with room does the
    # work more cheaply than a predecessor without.
    ctx="$(context_pct)"
    if [ "$ctx" != "unknown" ] && [ "$ctx" -ge 55 ]; then
      echo "ORCHESTRATOR IDLE at ${ctx}% context, ${remaining} boxes open. Do NOT start a package here — waking a context this large is expensive. Write the checkpoint and hand off to a fresh session: .agent-runtime/self-continue.sh context"
    else
      echo "ORCHESTRATOR IDLE — nothing running, no worker waiting, ${remaining} boxes still open. Pick up the next package, or touch .agent-runtime/stopped if this is deliberate."
    fi
    nudged=1
    # Back off hard after the first nudge: repeating every 8 minutes would be
    # noise, and the point is to restart a stopped loop, not to hector.
    sleep 1500
    quiet=0
    nudged=0
  fi
done
