#!/usr/bin/env bash
# scripts/board-watch.sh -- ONE Monitor for the whole board.
#
# WHY THIS EXISTS
# ---------------
# CLAUDE.md arms four persistent watches per session (worker-watch.sh per
# worker, prompt-watch.sh, pipeline.sh --watch, stale-workspaces.sh --watch),
# plus one per CI run checked by hand. Each fires per event, and Claude
# Code's Monitor primitive turns every stdout line into its own turn -- so an
# orchestrator supervising five workers and a CI matrix spends its context
# triaging one event at a time while workers wait. Measured 2026-09-05: the
# Fable 5.1 session (four monitors, dozens of events, a planted-panic filter
# restarted mid-run) and the primary glasshouse-78 the same evening (eleven
# monitor events in a row before one act).
#
# This composes the same four scripts' own detection -- it reimplements none
# of it -- into ONE digest line per window, plus an immediate `!` line for a
# small, enumerated interrupt class. The four scripts are untouched and
# remain valid armed on their own; this is swappable, not a replacement.
#
# USAGE
#   scripts/board-watch.sh --window 120 \
#     --worker pane-spec:surface:20:/abs/.agent-runtime/report-pane-spec.md \
#     --worker cache-temperature:surface:18:/abs/.agent-runtime/report-cache-temperature.md \
#     --ci 33971282609 [--ci ...] \
#     [--self workspace:N] [--tick 20] [--heartbeat 10] [--once]
#
# --worker is repeatable, colon-joined name:surface-kind:surface-num:report
# (mirrors worker-watch.sh's <name> <surface-ref> <report> triple -- the
# surface ref itself contains a colon, e.g. "surface:20", so the split is
# exactly four fields, not three).
# --ci is repeatable (a GitHub Actions run id).
# --self is passed through to the prompt sweep as PROMPT_WATCH_SELF, so the
# orchestrator's own pane is not read back as a stuck prompt.
# --once prints one digest and exits 0 -- for the test below, and for a human
# who wants a single snapshot without arming anything.
#
# INTERRUPT CLASS -- exactly five kinds, nothing else jumps the window:
#   ! REPORT <name> <path>                  a worker's report file appeared
#   ! PROMPT <ws> "Do you want to proceed?" a pane is stuck on a permission prompt
#   ! QUIET <name> <surface> — no done signal   a worker went quiet with nothing to show for it
#   ! CI <id> FAILURE                       a run's overall conclusion became failure
#   ! DRY pipeline below floor (N live)     the board dropped below pipeline.sh's floor
# A CI run completing green, a worker still busy, a stale pane, or a CI cell
# landing all wait for the digest -- see the packet this script was written
# from (GH-BOARD-WATCH) for why these five and not more.
set -uo pipefail

# .agent-runtime/ is anchored to the ONE main checkout, the same trap every
# other watch in this project guards against (see worker-watch.sh's and
# pipeline.sh's own header comments) -- BASH_SOURCE alone silently answers
# about whichever tree happens to be executing this copy of the script.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAIN_COMMON="$(git -C "$SCRIPT_DIR" rev-parse --git-common-dir 2>/dev/null)"
case "$MAIN_COMMON" in
  /*) : ;;
  *)  MAIN_COMMON="$(cd "$SCRIPT_DIR/$MAIN_COMMON" 2>/dev/null && pwd -P)" ;;
esac
if [ -z "$MAIN_COMMON" ] || [ "$(basename "$MAIN_COMMON")" != ".git" ]; then
  echo "board-watch: refusing -- not running from a checkout of this repository" >&2
  exit 1
fi
REPO="$(dirname "$MAIN_COMMON")"

# The four composed scripts, as full paths -- overridable so
# tests/test_board_watch.py can point pipeline.sh/stale-workspaces.sh at
# small fixture scripts instead of asking the real board state (live
# worktrees, live gh runs) to hold still for a test. worker-watch.sh and
# prompt-watch.sh are exercised against their REAL selves in the test, with
# only `cmux`/`gh` faked on PATH (the same pattern test_worker_watch.py
# already uses), because their behaviour IS the thing under test.
#
# worker-watch.sh, pipeline.sh and stale-workspaces.sh are always the MAIN
# CHECKOUT's copies: worker-watch.sh and pipeline.sh do their own
# git-common-dir anchoring so it would not matter which copy ran, but
# stale-workspaces.sh does NOT -- it derives its own "REPO" straight from
# its BASH_SOURCE with no anchoring at all, so running a worktree's copy of
# it silently checks worktree-relative paths that do not exist and misreads
# every live worker as stale. Only the main checkout's own copy is correct.
# prompt-watch.sh is the one exception: it never reads a repo path (it only
# ever talks to cmux), so there is nothing to anchor -- and reaching across
# to the main checkout's copy would run without PROMPT_WATCH_ONCE until this
# packet's diff is integrated there, which is exactly what hung the first
# version of this script. SCRIPT_DIR (this script's own location) always
# ships the matching feature.
WORKER_WATCH_SH="${BOARD_WATCH_WORKER_WATCH_SH:-$REPO/scripts/worker-watch.sh}"
PROMPT_WATCH_SH="${BOARD_WATCH_PROMPT_WATCH_SH:-$SCRIPT_DIR/scripts/prompt-watch.sh}"
PIPELINE_SH="${BOARD_WATCH_PIPELINE_SH:-$REPO/scripts/pipeline.sh}"
STALE_WORKSPACES_SH="${BOARD_WATCH_STALE_WORKSPACES_SH:-$REPO/scripts/stale-workspaces.sh}"

WINDOW=120
TICK=20
HEARTBEAT=10
SELF=""
ONCE=0
WORKER_SPECS=()
CI_IDS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --window) WINDOW="$2"; shift 2 ;;
    --tick) TICK="$2"; shift 2 ;;
    --heartbeat) HEARTBEAT="$2"; shift 2 ;;
    --self) SELF="$2"; shift 2 ;;
    --worker) WORKER_SPECS+=("$2"); shift 2 ;;
    --ci) CI_IDS+=("$2"); shift 2 ;;
    --once) ONCE=1; shift ;;
    *) echo "board-watch: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

# Every line below is written with printf, a bash builtin: each call is its
# own write(2), so output is line-buffered onto a pipe by construction and a
# Monitor sees each line as it is written -- no stdbuf wrapper needed.

now_hm() { date +%H:%M; }

# ---------------------------------------------------------------------------
# WORKERS -- source worker-watch.sh's own pane classification per worker.
# WORKER_WATCH_TEST_SOURCE=1 is the hook the file already exposes for exactly
# this: it defines is_busy_screen/is_never_started/worker_signalled/etc as
# functions and returns before starting its own loop, without requiring a
# real invocation's positional args. No edit to worker-watch.sh was needed --
# see the report for what this reused instead of reimplementing.
worker_state_line=""   # name:state pairs space-joined, current tick
declare -A prev_worker_state   # name -> busy|report|quiet
declare -A worker_quiet_count  # name -> consecutive quiet-no-signal ticks

collect_workers() {
  local busy=0 quiet=0 report_names=() spec name kind num report surface screen state
  for spec in "${WORKER_SPECS[@]+"${WORKER_SPECS[@]}"}"; do
    IFS=':' read -r name kind num report <<<"$spec"
    surface="$kind:$num"
    # A command substitution runs the sourcing and classification in ITS OWN
    # subshell, which is fine -- only the printed word crosses back out. Doing
    # this as a `| { ... }` pipeline instead (an earlier draft) would have run
    # the state-accounting block in a subshell too, silently discarding every
    # update to busy/quiet/report_names/the associative arrays below: bash
    # forks a subshell for EVERY stage of a pipeline, last stage included.
    state="$(
      WORKER_WATCH_TEST_SOURCE=1 source "$WORKER_WATCH_SH" "$name" "$surface" "$report" >/dev/null 2>&1
      if [ -f "$report" ]; then
        echo "report"
      else
        screen="$(cmux read-screen --surface "$surface" 2>/dev/null)"
        if is_busy_screen "$screen"; then
          echo "busy"
        else
          echo "quiet"
        fi
      fi
    )"
    case "$state" in
      report) report_names+=("$name") ;;
      busy)   busy=$((busy+1)) ;;
      quiet)  quiet=$((quiet+1)) ;;
    esac

    # Interrupts, edge-triggered against the PREVIOUS tick's state for this
    # worker, printed immediately -- before any digest logic runs this tick.
    if [ "$state" = "report" ] && [ "${prev_worker_state[$name]:-}" != "report" ]; then
      printf '! %s REPORT %s %s\n' "$(now_hm)" "$name" "$report"
    fi
    if [ "$state" = "quiet" ]; then
      worker_quiet_count[$name]=$(( ${worker_quiet_count[$name]:-0} + 1 ))
      # Two consecutive quiet reads before believing it -- the same debounce
      # worker-watch.sh itself uses, so a gap between tool calls is not
      # announced as a stuck worker.
      if [ "${worker_quiet_count[$name]}" -eq 2 ] && [ ! -f "$REPO/.agent-runtime/done/$name" ]; then
        printf '! %s QUIET %s %s — no done signal\n' "$(now_hm)" "$name" "$surface"
      fi
    else
      worker_quiet_count[$name]=0
    fi
    prev_worker_state[$name]="$state"
  done
  local names=""
  [ "${#report_names[@]}" -gt 0 ] && names="(${report_names[*]// /,})"
  worker_state_line="workers $busy busy · ${#report_names[@]} report${names} · $quiet quiet"
}

# ---------------------------------------------------------------------------
# PROMPTS -- one sweep of prompt-watch.sh, report-only, never approving.
# SECURITY / ISOLATION INVARIANT (packet): this script observes prompts and
# never presses Enter for anyone. PROMPT_WATCH_APPROVE=0 is forced here, not
# merely defaulted, so composing this watch can never widen prompt-watch.sh's
# own approval behaviour.
prompt_line=""
declare -A prev_prompt_ws

collect_prompts() {
  local out ws_list ws count=0
  # Unlike the other three, prompt-watch.sh does no repo anchoring at all --
  # it only ever talks to cmux, never a path under $REPO -- so there is no
  # reason to reach across to the main checkout's copy, and doing so would
  # silently run without PROMPT_WATCH_ONCE until that copy is integrated.
  # SCRIPT_DIR (this script's own location) always has the matching feature.
  out="$(PROMPT_WATCH_APPROVE=0 PROMPT_WATCH_ONCE=1 PROMPT_WATCH_SELF="${SELF:-none}" "$PROMPT_WATCH_SH" 2>/dev/null)"
  # ONE match per PROMPT line, not per occurrence: prompt-watch.sh's own
  # message names the workspace TWICE ("PROMPT $ws is waiting ... approve
  # with: cmux send-key --workspace $ws Enter"), so a plain grep across the
  # whole line counted every real prompt twice.
  ws_list="$(printf '%s\n' "$out" | /usr/bin/grep '^PROMPT ' | while IFS= read -r pl; do
    printf '%s\n' "$pl" | /usr/bin/grep -oE 'workspace:[0-9]+' | head -1
  done)"
  declare -A seen_this_tick
  while IFS= read -r ws; do
    [ -z "$ws" ] && continue
    seen_this_tick[$ws]=1
    count=$((count+1))
    if [ -z "${prev_prompt_ws[$ws]:-}" ]; then
      printf '! %s PROMPT %s "Do you want to proceed?"\n' "$(now_hm)" "$ws"
    fi
  done <<<"$ws_list"
  # Clear resolved prompts so a re-appearance interrupts again.
  local k
  for k in "${!prev_prompt_ws[@]}"; do
    [ -z "${seen_this_tick[$k]:-}" ] && unset 'prev_prompt_ws[$k]'
  done
  for k in "${!seen_this_tick[@]}"; do prev_prompt_ws[$k]=1; done
  prompt_line="prompts $count"
}

# ---------------------------------------------------------------------------
# CI -- gh run view per --ci id. jobs completed/total drives the in-progress
# fraction; a completed run reports its conclusion. FAILURE interrupts on the
# edge into "failure"; every other conclusion (including a red CELL inside an
# otherwise still-running matrix) waits for the digest, per the packet.
ci_line=""
declare -A prev_ci_conclusion

collect_ci() {
  local parts=() id json status conclusion done_jobs total_jobs
  for id in "${CI_IDS[@]+"${CI_IDS[@]}"}"; do
    json="$(gh run view "$id" --json status,conclusion,jobs 2>/dev/null)"
    if [ -z "$json" ]; then
      parts+=("$id unknown")
      continue
    fi
    status="$(printf '%s' "$json" | jq -r '.status // "unknown"')"
    conclusion="$(printf '%s' "$json" | jq -r '.conclusion // "null"')"
    if [ "$status" = "completed" ]; then
      parts+=("$id $conclusion")
    else
      done_jobs="$(printf '%s' "$json" | jq '[.jobs[]? | select(.status=="completed")] | length')"
      total_jobs="$(printf '%s' "$json" | jq '.jobs | length')"
      parts+=("$id $status $done_jobs/$total_jobs")
    fi
    if [ "$conclusion" = "failure" ] && [ "${prev_ci_conclusion[$id]:-}" != "failure" ]; then
      printf '! %s CI %s FAILURE\n' "$(now_hm)" "$id"
    fi
    prev_ci_conclusion[$id]="$conclusion"
  done
  if [ "${#parts[@]}" -gt 0 ]; then
    ci_line="ci $(IFS=' · '; echo "${parts[*]}")"
  else
    ci_line="ci none"
  fi
}

# ---------------------------------------------------------------------------
# DRY -- pipeline.sh --check is already one-shot: it prints its report and
# exits 1 below the floor, 0 at or above it.
dry_line=""
prev_dry=0

collect_dry() {
  local out live dry_now=0
  out="$("$PIPELINE_SH" --check 2>/dev/null)"
  if [ $? -ne 0 ]; then dry_now=1; fi
  live="$(printf '%s' "$out" | /usr/bin/grep -oE 'live=[0-9]+' | head -1 | cut -d= -f2)"
  live="${live:-0}"
  if [ "$dry_now" -eq 1 ]; then
    dry_line="dry yes"
    if [ "$prev_dry" -eq 0 ]; then
      printf '! %s DRY pipeline below floor (%s live)\n' "$(now_hm)" "$live"
    fi
  else
    dry_line="dry no"
  fi
  prev_dry="$dry_now"
}

# ---------------------------------------------------------------------------
# STALE -- stale-workspaces.sh with no args is already one-shot: it lists
# STALE lines (or "none") and exits accordingly. Count only, per the
# packet -- a stale pane waits for the digest, it never interrupts.
stale_line=""

collect_stale() {
  local out n
  out="$("$STALE_WORKSPACES_SH" 2>/dev/null)"
  n="$(printf '%s\n' "$out" | /usr/bin/grep -c '^STALE ' || true)"
  stale_line="stale $n"
}

# ---------------------------------------------------------------------------
digest_line() {
  printf 'BOARD %s %s | %s | %s | %s | %s' \
    "$(now_hm)" "$worker_state_line" "$prompt_line" "$ci_line" "$dry_line" "$stale_line"
}

last_digest_body=""
windows_since_digest=0
elapsed_since_window=0

tick() {
  collect_workers
  collect_prompts
  collect_ci
  collect_dry
  collect_stale
}

emit_if_due() {
  local body line
  body="$worker_state_line|$prompt_line|$ci_line|$dry_line|$stale_line"
  line="$(digest_line)"
  if [ "$body" != "$last_digest_body" ]; then
    printf '%s\n' "$line"
    last_digest_body="$body"
    windows_since_digest=0
  else
    windows_since_digest=$((windows_since_digest+1))
    if [ "$windows_since_digest" -ge "$HEARTBEAT" ]; then
      printf '%s\n' "$line"
      windows_since_digest=0
    fi
  fi
}

if [ "$ONCE" -eq 1 ]; then
  tick
  emit_if_due
  exit 0
fi

# Prime the digest baseline with one real observation before the loop
# starts, WITHOUT printing it. A board that starts quiet and stays quiet
# must print nothing for the first HEARTBEAT-1 windows (acceptance test 3) --
# comparing the first real window against an empty sentinel would make that
# first window always look "changed" and print immediately, which is not a
# digest anyone asked for. Interrupts are unaffected by priming: they compare
# against each per-item prev_* map, which starts empty regardless, so a
# worker that already has a report waiting, or a pane already stuck on a
# prompt, still interrupts on this very first observation -- priming only
# suppresses the one thing that has no "true alarm" case: the digest itself.
tick
last_digest_body="$worker_state_line|$prompt_line|$ci_line|$dry_line|$stale_line"

while true; do
  sleep "$TICK"
  tick
  elapsed_since_window=$((elapsed_since_window + TICK))
  if [ "$elapsed_since_window" -ge "$WINDOW" ]; then
    elapsed_since_window=0
    emit_if_due
  fi
done
