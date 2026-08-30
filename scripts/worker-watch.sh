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

# WORKER_WATCH_TEST_SOURCE=1 lets scripts/tests/test_worker_watch.py `source`
# this file to call its pure pane-classification functions (is_busy_screen,
# is_never_started, has_worker_output, last_words_from) directly against
# synthetic screens, without a real worker name/surface/report and without
# starting the watch loop at the bottom of the file. A real invocation is
# unaffected -- this only relaxes which arguments are required and adds one
# early-return guard right before the loop starts (search TEST_SOURCE below).
TEST_SOURCE="${WORKER_WATCH_TEST_SOURCE:-0}"

if [ "$TEST_SOURCE" = "1" ]; then
  NAME="${1:-test-worker}"
  SURFACE="${2:-test-surface}"
  REPORT="${3:-/dev/null}"
  NAG="${4:-180}"
else
  NAME="${1:?usage: worker-watch.sh <name> <surface-ref> <report-path> [nag-seconds]}"
  SURFACE="${2:?missing surface ref}"
  REPORT="${3:?missing report path}"
  NAG="${4:-180}"
fi

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Where this worker's worktree lives, for the growth signal below. Defaults to
# this project's own convention -- `glasshouse-<name>` beside the main checkout,
# the same derivation `scripts/hooks/worker-turn-ended.sh` already relies on.
WORKTREE="${5:-$REPO/.worktrees/$NAME}"
[ -e "$WORKTREE" ] || [ ! -e "$REPO/../glasshouse-$NAME" ] || WORKTREE="$REPO/../glasshouse-$NAME"
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
#
# Matched CASE-INSENSITIVELY (see is_busy_screen): a real pane rendered
# `Esc to cancel` with a capital E while this pattern had lowercase only, and
# the capitalised form read as idle.
#
# The verb before the ellipsis rotates — Photosynthesizing, Discombobulating,
# Flowing, Flibbertigibbeting, Tinkering, Swooping, Baked, and whatever comes
# next — so matching specific gerunds is unwinnable, and was never the
# reliable part. What is reliable, per its own header line, is a leading
# spinner glyph (any symbol that is not a tool-output marker) immediately
# followed by one bare word and an ellipsis — `✻ Flowing…` — whether or not a
# parenthesised timer has appeared yet. `⎿ Tool result: ...(truncated)` does
# NOT match: the ellipsis there does not sit directly after the first word.
BUSY_RE='esc to interrupt|esc to cancel|[0-9]+s · ↓|\([0-9]+[hms]|[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]|will retry in|Retrying|Waiting for API response|API Error.*retry|^[[:space:]]*[^[:space:][:alnum:]⎿─❯][[:space:]]+[A-Za-z]+(…|\.\.\.)'

# The last line that says something, for the idle announcement. A notification
# that carries the pane's own last words would have caught both false idles
# without anyone opening the pane.
#
# Takes the already-read screen text rather than reading the pane again: a
# fresh `cmux read-screen` call here would read a DIFFERENT moment than
# whatever is_busy/is_never_started just judged, which is exactly the kind of
# gap §57's addenda warn about — a check must not report on state it did not
# itself establish.
last_words_from() {
  local screen="$1"
  # Skip the status bar as well as the blanks and rules: it is drawn last, so a
  # naive `tail -1` quotes `/rc` and tells the reader nothing. What is wanted is
  # the last thing the WORKER said.
  printf '%s\n' "$screen" \
    | grep -vE '^\s*$|^─+$|^\s*❯\s*$|^\s*/rc\s*$|auto mode on|^\s*Opus |^\s*Sonnet |[░█]{6}|^\s*⎿|Tip: Use|remote-control is active' \
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

# Kept as a no-arg call (`if is_busy; then`) because test_worker_signal.py
# greps for that exact call site as the guarantee that the pane, not the
# marker, gates. is_busy_screen below is the same test against an
# already-read screen, for the places that must not read the pane twice.
is_busy() {
  local screen
  screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)" || return 1
  is_busy_screen "$screen"
}

is_busy_screen() {
  local screen="$1"
  # Finished beats working. A pane showing its completion line is done even if
  # something above it still looks like activity. Case-insensitive: the
  # harness does not render these consistently (see BUSY_RE's header).
  printf '%s' "$screen" | grep -qiE "$DONE_RE" && return 1
  printf '%s' "$screen" | grep -qiE "$BUSY_RE"
}

# NEVER STARTED, distinct from every other quiet reading.
#
# Three workers on 2026-08-29 sat at an empty prompt for five minutes, and
# this watch called it "pane went quiet, NO report" three separate times. The
# orchestrator dismissed all three as the known false-idle-on-startup-banner
# case, because the message read exactly like the benign one. It was a real
# failure each time: the prompt never landed.
#
# NOT keyed on the status line. An earlier version of this required a `0/1M`
# token count and `~$0.00` cost reading — but those strings are the reporting
# integrator's OWN personal statusline configuration, not something this
# harness guarantees. Coupling detection to one user's statusline means it
# silently reverts to the false-idle behaviour this exists to remove the
# moment anyone's statusline differs or changes — the same failure shape as a
# check that quietly matches nothing and reports PASSED.
#
# Use the transcript instead of the chrome: a worker that received its prompt
# produces OUTPUT in the pane body; a worker that did not shows only the
# startup banner and an empty prompt. So never-started == no worker output at
# all, once the banner, the prompt line, the rules, the blanks and the status
# bar are filtered out.
#
# NOT the same filter as last_words_from. Measured 2026-08-30: three workers
# launched by new-worker.sh were flagged NEVER STARTED ~20 times each, all
# false — every one was mid-tool-call (its very first action is the ARM
# instruction's Monitor call), and a pane doing that shows only a `⎿` tool
# line, nothing else. last_words_from strips `⎿` lines on purpose, because
# quoting a truncated tool result as the worker's "last words" is unreadable
# — but that same strip, reused here, threw away the one thing on screen that
# proved the worker had started. Detecting "did anything happen" and
# rendering "what did it last say" are different questions; they now have
# different filters, and this one keeps the `⎿` line.
STARTUP_ONLY_RE='^\s*$|^─+$|^\s*❯\s*$|^\s*/rc\s*$|auto mode on|^\s*Opus |^\s*Sonnet |[░█]{6}|Tip: Use|remote-control is active'

has_worker_output() {
  local screen="$1"
  printf '%s\n' "$screen" | grep -vE "$STARTUP_ONLY_RE" | grep -q .
}

is_never_started() {
  local screen="$1"
  ! has_worker_output "$screen"
}

# §28's growth signal: a worktree that changed since the last read is being
# worked in, whatever its pane is drawing.
#
# The case this exists for is a **team lead**, which spends much of its batch
# waiting on subcontractors and is therefore legitimately quiet for long
# stretches. On 2026-08-26 `lead-extract` was announced "idle with NO report"
# forty minutes in, with five new files in `memory/extract/` and a
# subcontractor's findings just relayed. Nothing was wrong, and acking a false
# idle ENDS the watch -- so a false positive costs coverage rather than merely
# being noise.
#
# `git status --porcelain` respects `.gitignore`, so `target/` cannot trigger
# this: a background build is not mistaken for progress. `diff --shortstat` is
# paired with it because porcelain alone sees a file appear or vanish but not a
# file still being edited.
#
# This only ever DELAYS an announcement. A worker that has genuinely stopped
# leaves its tree still, the fingerprint stops moving, and the next quiet read
# announces as before.
# The worker's CUMULATIVE TOKEN COUNT, read off the status bar (`· 76k/1M`).
#
# Why this is here, and why the §28 worktree signal was not enough. §28's fix --
# "a pane whose worktree has grown since the last read is working" -- closes the
# false idle for a worker that is EDITING. It cannot close it for one that is
# THINKING, and the note that proposed it said so: *"Where it does not help: a
# worker thinking for a long time without writing."*
#
# Measured 2026-08-29, batch 47: `extract-switch`, a tests-only worker, was
# announced DONE three times while its own screen read
# `Boondoggling... (3m 48s - 2.6k tokens - thinking with high effort)`. It had
# written nothing yet, so its worktree fingerprint never moved, and two quiet
# frames were enough. Acking it to silence the nag ENDED ITS WATCH while it was
# still working, which is the more expensive half of the bug.
#
# The token counter is the signal such a worker cannot fail to emit: it is in
# the status line every frame, it only ever goes up while the model is running,
# and it is orthogonal to every spinner glyph -- which is the trap §57's third
# addendum warns about, where each fix is one more string to enumerate.
token_reading() {
  cmux read-screen --surface "$SURFACE" 2>/dev/null \
    | grep -oE '[0-9]+(\.[0-9]+)?k?/1M' | head -1
}

worktree_fingerprint() {
  local tokens
  tokens="$(token_reading)"
  [ -d "$WORKTREE" ] || { printf 'no-worktree-%s' "$tokens"; return; }
  {
    git -C "$WORKTREE" status --porcelain -uall 2>/dev/null
    git -C "$WORKTREE" diff --shortstat 2>/dev/null
    printf 'tokens:%s' "$tokens"
  } | cksum
}

if [ "$TEST_SOURCE" = "1" ]; then
  # Test source: functions are defined, nothing else runs. See TEST_SOURCE
  # above.
  return 0 2>/dev/null || exit 0
fi

quiet=0
announced=0
announced_kind=""   # "done" or "never-started"
growth_noted=0
# Latched true the moment ANY real evidence of activity is seen for this
# worker: a busy pane, worktree growth, or a retraction (below). Once true it
# stays true for the life of this watch process — a worker cannot un-start.
# Without this, a worker that was correctly retracted from a false
# NEVER STARTED could still hit a second quiet-and-empty read later (idle
# between tool calls looks the same on screen as idle before the first one)
# and get flagged NEVER STARTED again, with no new evidence that the claim is
# true the second time.
ever_started=0
last_fingerprint="$(worktree_fingerprint)"

while true; do
  sleep 20

  if [ "$announced" -eq 1 ]; then
    if [ ! -f "$MARKER" ]; then
      echo "acknowledged: worker '$NAME' ticked off; watch ending"
      exit 0
    fi
    # A never-started worker can be re-sent its prompt. If it starts
    # producing, the stale announcement is dropped and normal watching
    # resumes — otherwise a fixed misfire nags forever as if nothing changed.
    # One read here serves both this check and the message below it.
    ann_screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)"
    if [ "$announced_kind" = "never-started" ] && is_busy_screen "$ann_screen"; then
      announced=0
      announced_kind=""
      quiet=0
      ever_started=1
      rm -f "$MARKER"
      echo "NOTE  '$NAME' started producing output after being flagged NEVER STARTED — resuming normal watch"
      continue
    fi
    now=$(date +%s)
    stamped=$(cat "$MARKER" 2>/dev/null || echo "$now")
    if [ $(( now - stamped )) -ge "$NAG" ]; then
      date +%s > "$MARKER"
      if [ "$announced_kind" = "never-started" ]; then
        echo "WORKER NEVER STARTED: '$NAME' — the prompt did not land; re-send it (surface $SURFACE)"
      elif [ -f "$REPORT" ]; then
        echo "STILL UNACKNOWLEDGED: '$NAME' idle with a report waiting at $REPORT — review it, then: scripts/worker-ack.sh $NAME"
      else
        echo "STILL UNACKNOWLEDGED: '$NAME' idle and wrote NO report — its last line was: $(last_words_from "$ann_screen") — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
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
    ever_started=1
    last_fingerprint="$(worktree_fingerprint)"
    continue
  fi

  # Quiet pane, but is the worktree moving? See worktree_fingerprint above.
  fingerprint="$(worktree_fingerprint)"
  if [ "$fingerprint" != "$last_fingerprint" ]; then
    last_fingerprint="$fingerprint"
    quiet=0
    ever_started=1
    if [ "$growth_noted" -eq 0 ]; then
      growth_noted=1
      echo "NOTE  '$NAME' pane is quiet but its worktree or its token count is still moving — treating it as working (§28, plus the thinking-worker case). This note fires once."
    fi
    continue
  fi

  quiet=$(( quiet + 1 ))

  # Two consecutive quiet reads before believing it. One catches the gap
  # between a worker's tool calls and cries wolf.
  if [ "$quiet" -ge 2 ]; then
    # One read serves the busy recheck below, the never-started test, and the
    # message either branch feeds.
    quiet_screen="$(cmux read-screen --surface "$SURFACE" 2>/dev/null)"
    if [ -z "$quiet_screen" ]; then
      # §54: a failed or empty read means "cannot see this pane right now,"
      # not "the pane is empty." Collapsing the two would let a transient
      # cmux hiccup announce, confidently, that the prompt never landed. Skip
      # this tick without touching quiet/announced state; the next read is
      # 20s away.
      continue
    fi
    # Re-check busy-ness on THIS SAME screen, immediately before deciding.
    # `is_busy` above read the pane at the top of this iteration — a
    # different moment, one or two more `cmux read-screen` calls back — and
    # only that earlier read fed the quiet counter. Deciding never-started
    # from a later, un-rechecked read let a pane that had since started
    # producing output (e.g. its first spinner appearing) still read as
    # silent. The retraction path below already trusts is_busy_screen as the
    # authority for "is this worker actually working"; the pre-announce path
    # now does too, on the exact screen it is about to act on.
    if is_busy_screen "$quiet_screen"; then
      quiet=0
      ever_started=1
      last_fingerprint="$(worktree_fingerprint)"
      continue
    fi
    date +%s > "$MARKER"
    announced=1
    # ever_started latches once real activity has ever been seen (busy,
    # worktree growth, or a prior retraction — see where each is set above).
    # A worker that has already proven it started cannot un-start, so a
    # later quiet-and-empty read is read as ordinary idleness, not as
    # evidence the prompt never landed a second time.
    if [ "$ever_started" -eq 0 ] && is_never_started "$quiet_screen"; then
      announced_kind="never-started"
      echo "WORKER NEVER STARTED: '$NAME' — the prompt did not land; re-send it (surface $SURFACE)"
    else
      announced_kind="done"
      if [ -f "$REPORT" ]; then
        echo "WORKER DONE: '$NAME' — $(signal_kind), report present at $REPORT — review, then: scripts/worker-ack.sh $NAME"
      else
        echo "WORKER DONE: '$NAME' — $(signal_kind), but NO report — its last line was: $(last_words_from "$quiet_screen") — inspect $SURFACE, then: scripts/worker-ack.sh $NAME"
      fi
    fi
  fi
done
