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

# A pane is BUSY when its screen shows a spinner, an interrupt hint, a running
# token counter, or a **retry countdown**. Anything else is idle.
#
# The retry case was paid for on 2026-08-27. A worker mid-package hit
# `Waiting for API response · will retry in 2m 8s`, which draws no spinner and
# no counter, and this watch announced "went idle with NO report". It had not
# finished, it had not stopped, and it had not asked anything — it was waiting.
# A watch that reports a state it has not established is the same defect as a
# report nobody reads (practice §57's addendum), pointing the other way.
#
# Read the SURFACE, never the workspace: a workspace ref can resolve to a
# sibling pane, and an empty ref silently resolves to the caller's own pane —
# which once nearly sent a stray keystroke to the orchestrator itself.
# The signals, enumerated rather than implied. Two false idles were paid for on
# 2026-08-27: a retry countdown (no spinner, no counter), and a spinner glyph
# from a set this pattern did not list. Adding one string at a time is how the
# second one happened, so the third signal here is deliberately generic — a
# parenthesised elapsed timer, which every working state draws whatever glyph
# it picks. `(shift+tab to cycle)` and the unparenthesised `1h34m` in the status
# bar do not match it.
# A POSITIVE signal that the work is over, checked BEFORE the busy signals and
# overriding them. Claude Code prints its completion line with the SAME star
# glyph it spins with — `✻ Churned for 35m 7s · done 8:42 AM` — so a glyph can
# never decide this, and on 2026-08-27 a glyph did: two finished workers sat
# unreported for 35 and 47 minutes while the watch called them busy. The word
# is `done`, and it is the only thing in that line that means it.
DONE_RE='(Churned|Worked|Ran|Thought) for .*· *done |· *done [0-9]+:[0-9]{2}'

# BUSY, checked second. Deliberately free of glyphs: every spinner character
# this harness draws is also used in the completion line above, so the reliable
# signal is a **parenthesised elapsed timer**, which only a working state draws.
# `(shift+tab to cycle)` does not match it, `Churned for 35m 7s` does not match
# it (no parenthesis), and the status bar's own `1h34m` does not match it.
BUSY_RE='esc to interrupt|esc to cancel|[0-9]+s · ↓|\([0-9]+[hms]|[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]|will retry in|Retrying|Waiting for API response|API Error.*retry'

# The last line that says something, for the idle announcement. A notification
# that carries the pane's own last words would have caught both false idles
# without anyone opening the pane.
last_words() {
  # Skip the status bar as well as the blanks and rules: it is drawn last, so a
  # naive `tail -1` quotes `/rc` and tells the reader nothing. What is wanted is
  # the last thing the WORKER said.
  cmux read-screen --surface "$SURFACE" 2>/dev/null \
    | grep -vE '^\s*$|^─+$|^\s*❯\s*$|^\s*/rc\s*$|auto mode on|^\s*Opus |^\s*Sonnet |[░█]{6}|^\s*⎿|Tip: Use' \
    | tail -1 \
    | sed 's/^[[:space:]]*//' \
    | cut -c1-110
}

# The authoritative signal: the worker said so itself, via worker-done.sh.
# Checked before anything is read off the pane, because a worker that has
# spoken needs no interpretation.
DONE_FILE="$REPO/.agent-runtime/done/$NAME"

worker_signalled() {
  [ -f "$DONE_FILE" ]
}

# Which of the two signals fired. They mean different things: the worker
# saying so means the work is finished, while a quiet pane means only that
# nothing is drawing — it could be a crash, a kill, or a worker that stopped
# to ask something.
signal_kind() {
  if worker_signalled; then
    printf 'it signalled done (%s)' "$(cut -f2 "$DONE_FILE" 2>/dev/null | head -1)"
  else
    printf 'pane went quiet, NO done signal — it may have died or be waiting'
  fi
}

is_busy() {
  local screen
  screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)" || return 1
  # Finished beats working. A pane showing its completion line is done even if
  # something above it still looks like activity.
  printf '%s' "$screen" | grep -qE "$DONE_RE" && return 1
  printf '%s' "$screen" | grep -qE "$BUSY_RE"
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
        echo "STILL UNACKNOWLEDGED: '$NAME' idle and wrote NO report — its last line was: $(last_words) — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
      fi
    fi
    continue
  fi

  # BUSY ALWAYS WINS, and the done marker never overrides it.
  #
  # The Stop hook fires at the end of every model TURN, not at the end of the
  # work — `harness-hook-protocol.md` says so in as many words: "turn.completed
  # — a model turn became idle; work may or may not be done." The first version
  # of this treated the marker as authoritative, so the first turn boundary
  # latched 'done' forever and the watch announced a worker that was 42 minutes
  # into its package and still running.
  #
  # A transient event stored as durable state is the defect. The two signals
  # compose the other way round: the pane says whether a turn is running RIGHT
  # NOW, and the marker says a turn has ended at least once — which is what
  # separates 'finished' from 'died before it ever spoke'.
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
      echo "WORKER DONE: '$NAME' — $(signal_kind), report present at $REPORT — review, then: scripts/worker-ack.sh $NAME"
    else
      echo "WORKER DONE: '$NAME' — $(signal_kind), but NO report — its last line was: $(last_words) — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
    fi
  fi
done
