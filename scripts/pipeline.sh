#!/usr/bin/env bash
# Report the orchestrator's pipeline depth, and nag when it runs dry.
#
# WHY THIS EXISTS
# ---------------
# On 2026-08-29 the user asked: *"one worker running — is it genuinely blocking,
# or could you build new work packages and send some off?"* It was not blocking.
# One worker held three paths; ~90% of the tree was unclaimed. The orchestrator
# had simply stopped feeding the board and had no mechanism that noticed.
#
# That is the same failure this project keeps paying for in other shapes: the
# continuity watch (§85's neighbour) existed because nobody notices their own
# context filling up, and `worker-watch.sh` exists because a finished worker is
# invisible until something nags. **An orchestrator does not notice an empty
# board either** — every existing watch fires on a worker *event*, and a board
# with no workers generates no events at all. It is quiet in exactly the way
# that looks like nothing is wrong.
#
# So this is the missing watch: the one that fires on the ABSENCE of work.
#
# WHAT IT COUNTS
#   live      worktrees under .worktrees/ — one per dispatched worker
#   waiting   workers idle with an unread report (`worker-ack.sh --list`)
#   ready     packets in .agent-runtime/ with no worktree of the same name,
#             i.e. written and validated but never sent
#
# THE FLOOR IS TWO, AND THAT IS A JUDGEMENT
# Practice §74 measures the ceiling as review collision, not quota: past three
# concurrent editing workers the orchestrator's own review becomes the
# bottleneck, and its review is what catches a mutation KILLED by the wrong
# assertion. So the target band is 2–3, and this script nags below 2 rather
# than below 1 — by the time the board is empty, the refill has already cost
# wall-clock that parallel work would have absorbed.
#
# USAGE
#   scripts/pipeline.sh                 # print the state once, exit 0
#   scripts/pipeline.sh --check         # exit 1 if below the floor (for CI/hooks)
#   scripts/pipeline.sh --watch [secs]  # emit a line ONLY when below the floor
#
# The --watch form is meant for Monitor:
#   Monitor(command: "scripts/pipeline.sh --watch 300", persistent: true)
# It stays silent while the board is healthy, so every line it emits is
# actionable — which is the property a nag needs to keep being read.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1
FLOOR="${PIPELINE_FLOOR:-2}"

snapshot() {
  LIVE=0; LIVE_NAMES=""
  if [ -d .worktrees ]; then
    for d in .worktrees/*/; do
      [ -d "$d" ] || continue
      LIVE=$((LIVE+1)); LIVE_NAMES="$LIVE_NAMES $(basename "$d")"
    done
  fi
  WAITING="$(scripts/worker-ack.sh --list 2>/dev/null | grep -cv 'no workers waiting' || true)"
  case "$WAITING" in ''|*[!0-9]*) WAITING=0;; esac
  # A packet is READY only if it has never been worked: no worktree (not
  # dispatched) and no report (not finished). Without the report test this
  # counted every packet in the project's history — 135 of them — because a
  # finished worker's worktree is removed once its diff is integrated and
  # pushed. A number that large is not a signal, it is wallpaper.
  READY=0; READY_NAMES=""
  for p in .agent-runtime/packet-*.md; do
    [ -f "$p" ] || continue
    n="$(basename "$p" .md)"; n="${n#packet-}"
    [ -d ".worktrees/$n" ] && continue
    [ -f ".agent-runtime/report-$n.md" ] && continue
    READY=$((READY+1)); READY_NAMES="$READY_NAMES $n"
  done
}

report() {
  printf '\033[1mpipeline\033[0m  live=%d  waiting=%d  ready-to-dispatch=%d  (floor %d)\n' \
    "$LIVE" "$WAITING" "$READY" "$FLOOR"
  [ -n "$LIVE_NAMES" ]  && printf '  live:  %s\n' "$LIVE_NAMES"
  [ -n "$READY_NAMES" ] && printf '  ready: %s\n' "$READY_NAMES"
  return 0
}

nag() {
  printf 'PIPELINE LOW: %d worker(s) running, floor is %d.' "$LIVE" "$FLOOR"
  if [ "$READY" -gt 0 ]; then
    printf ' %d packet(s) already written and never dispatched:%s' "$READY" "$READY_NAMES"
  else
    printf ' No packet is ready. Run scripts/cluster-b.py for candidates, read'
    printf ' docs/process/refusal-register.md BEFORE choosing (six of Phase 32A'
    printf ' 9 open lines are Cluster E, "do not package"), then'
    printf ' scripts/new-packet.sh.'
  fi
  printf ' A defect does not stop the line (practice §84).\n'
}

case "${1:-}" in
  --check) snapshot; report; [ "$LIVE" -ge "$FLOOR" ] || exit 1 ;;
  --watch)
    every="${2:-300}"
    while true; do
      snapshot
      [ "$LIVE" -lt "$FLOOR" ] && nag
      sleep "$every"
    done ;;
  *) snapshot; report ;;
esac
