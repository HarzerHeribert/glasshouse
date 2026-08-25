#!/usr/bin/env bash
# Watch one visible worker pane and NAG until the orchestrator ticks it off.
#
# WHY THIS EXISTS
# ---------------
# A worker going idle used to produce exactly one notification. If it arrived
# while the orchestrator was mid-thought on something else, it got read and not
# acted on — and then it was gone. On 2026-08-25 a worker was started with no
# watch at all and finished completely unnoticed; the user spotted it, not the
# orchestrator.
#
# So this does not fire once. Once the pane is idle it keeps reminding, every
# NAG_SECONDS, until someone physically ticks the item off:
#
#     scripts/worker-ack.sh <name>
#
# USAGE
#   scripts/worker-watch.sh <name> <surface-ref> <report-path> [nag-seconds]
#
# Drive it with the Monitor tool, persistent, ONE PER WORKER:
#   Monitor(command: "scripts/worker-watch.sh p09f surface:61 /abs/report.md",
#           persistent: true)
#
# Arm it in the SAME turn you start the worker. A worker without a watch is a
# worker you will forget.
set -u

NAME="${1:?usage: worker-watch.sh <name> <surface-ref> <report-path> [nag-seconds]}"
SURFACE="${2:?missing surface ref}"
REPORT="${3:?missing report path}"
NAG="${4:-180}"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IDLE_DIR="$REPO/.agent-runtime/idle"
MARKER="$IDLE_DIR/$NAME"
mkdir -p "$IDLE_DIR"

# A pane is BUSY when its screen shows a spinner, an interrupt hint, or a
# running token counter. Anything else is idle.
#
# Read the SURFACE, never the workspace: a workspace ref can resolve to a
# sibling pane, and an empty ref silently resolves to the caller's own pane —
# which once nearly sent a stray keystroke to the orchestrator itself.
is_busy() {
  local screen
  screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)" || return 1
  printf '%s' "$screen" | grep -qE 'esc to interrupt|esc to cancel|[0-9]+s · ↓|[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]'
}

quiet=0
announced=0

while true; do
  sleep 20

  if [ "$announced" -eq 1 ]; then
    if [ ! -f "$MARKER" ]; then
      echo "acknowledged: worker '$NAME' ticked off; watch ending"
      exit 0
    fi
    now=$(date +%s)
    stamped=$(cat "$MARKER" 2>/dev/null || echo "$now")
    if [ $(( now - stamped )) -ge "$NAG" ]; then
      date +%s > "$MARKER"
      if [ -f "$REPORT" ]; then
        echo "STILL UNACKNOWLEDGED: '$NAME' idle with a report waiting at $REPORT — review it, then: scripts/worker-ack.sh $NAME"
      else
        echo "STILL UNACKNOWLEDGED: '$NAME' idle and wrote NO report — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
      fi
    fi
    continue
  fi

  if is_busy; then
    quiet=0
    continue
  fi
  quiet=$(( quiet + 1 ))

  # Two consecutive quiet reads before believing it. One catches the gap
  # between a worker's tool calls and cries wolf.
  if [ "$quiet" -ge 2 ]; then
    date +%s > "$MARKER"
    announced=1
    if [ -f "$REPORT" ]; then
      echo "WORKER IDLE: '$NAME' finished, report present at $REPORT — review, then: scripts/worker-ack.sh $NAME"
    else
      echo "WORKER IDLE: '$NAME' went idle with NO report — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
    fi
  fi
done
