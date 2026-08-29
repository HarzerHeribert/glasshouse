#!/usr/bin/env bash
# Convergent co-editing: let two workers share a file without a lock.
#
# WHY THIS EXISTS
# ---------------
# Strict file partitioning ("never two workers on one file") is this project's
# oldest concurrency rule and it costs real parallelism. Measured on batch 45:
# **six of seven implementation packets carried a deferral instruction** — "this
# file is FORBIDDEN, write the patch into your report" — because a file the
# package needed belonged to another live worker. Each deferral converts worker
# work into orchestrator work at the one point that does not parallelise.
#
# The contended file is almost always `main.rs`, and that is structural:
# practice §32 says put the caller's file in the partition, and `main.rs` is
# where every production caller lives.
#
# THE PROTOCOL (practice §77, capability map Maybe L)
# --------------------------------------------------
# Two workers may share a file. Each still works ONLY in its own git worktree —
# that worktree IS the "pre-implementation buffer", and it must be a worktree
# rather than a patch file because an agent that cannot compile cannot verify,
# and verification is the agent's entire value here.
#
# Each reads the other's in-progress version ONCE, at finalization, adapts, and
# says what it changed because of what it saw. Then it declares done. When every
# claimant has declared, the barrier opens and the ORCHESTRATOR reconciles.
#
# READ ONCE, NOT CONTINUOUSLY. If A reads B, adapts, and B changes again, A is
# stale; unbounded mutual adaptation oscillates. One look terminates.
#
# NOTHING HERE WRITES TO ANOTHER WORKER'S TREE, and nothing writes to `main`.
# This tool only ever *reads* peers' worktrees and records claims under
# `.agent-runtime/coedit/`, which is gitignored.
#
# A PEER'S VERSION IS AN UNFINISHED PROPOSAL, NOT COMMITTED TRUTH. That warning
# is in the output on purpose: this project's recurring defect is a narrow,
# cited, plausible artifact read as more authoritative than it is (§75, §76).
#
# USAGE
#   coedit.sh claim   <file> <worker> [worktree]   register intent to edit
#   coedit.sh peers   <file> [worker]              who else, and where
#   coedit.sh diff    <file> [worker]              every peer's current diff
#   coedit.sh done    <file> <worker>              declare finished on this file
#   coedit.sh status  <file>                       barrier state
#   coedit.sh ready   <file>                       exit 0 iff all claimants done
#   coedit.sh list                                 every contended file
#   coedit.sh release <file>                       clear it after reconciliation
set -uo pipefail

REPO="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
ROOT="$REPO/.agent-runtime/coedit"

slug() { printf '%s' "$1" | sed 's#^\./##; s#[/ ]#__#g'; }
dir()  { printf '%s/%s' "$ROOT" "$(slug "$1")"; }

die() { printf 'coedit: %s\n' "$*" >&2; exit 2; }

cmd_claim() {
  local file="${1:?usage: claim <file> <worker> [worktree]}" worker="${2:?worker required}"
  local wt="${3:-$PWD}"
  [ -d "$wt" ] || die "worktree does not exist: $wt"
  mkdir -p "$(dir "$file")/claims" || die "cannot create state under $ROOT"
  printf '%s\n' "$wt" > "$(dir "$file")/claims/$worker"
  printf 'coedit: %s claimed %s (worktree %s)\n' "$worker" "$file" "$wt"
  cmd_peers "$file" "$worker"
}

cmd_peers() {
  local file="${1:?usage: peers <file> [worker]}" me="${2:-}"
  local d; d="$(dir "$file")"
  [ -d "$d/claims" ] || { printf 'coedit: no claims on %s\n' "$file"; return 0; }
  local found=0
  for c in "$d/claims"/*; do
    [ -e "$c" ] || continue
    local w; w="$(basename "$c")"
    [ "$w" = "$me" ] && continue
    found=$((found+1))
    local wt; wt="$(cat "$c")"
    local state="working"
    [ -e "$d/done/$w" ] && state="DONE on this file"
    printf '  peer %-22s %-12s %s\n' "$w" "$state" "$wt"
  done
  [ "$found" -eq 0 ] && printf 'coedit: no peers on %s — you are alone in it\n' "$file"
  return 0
}

cmd_diff() {
  local file="${1:?usage: diff <file> [worker]}" me="${2:-}"
  local d; d="$(dir "$file")"
  [ -d "$d/claims" ] || { printf 'coedit: no claims on %s\n' "$file"; return 0; }
  local any=0
  for c in "$d/claims"/*; do
    [ -e "$c" ] || continue
    local w; w="$(basename "$c")"
    [ "$w" = "$me" ] && continue
    any=1
    local wt; wt="$(cat "$c")"
    printf '\n===== %s — %s =====\n' "$w" "$file"
    if [ -d "$wt" ]; then
      git -C "$wt" diff -- "$file" 2>/dev/null || printf '(could not read)\n'
      git -C "$wt" status --porcelain -- "$file" 2>/dev/null | grep -q '^??' \
        && printf '(untracked in that worktree — new file)\n'
    else
      printf '(worktree gone: %s)\n' "$wt"
    fi
  done
  if [ "$any" -eq 1 ]; then
    cat <<'WARN'

-------------------------------------------------------------------------
This is an UNFINISHED PROPOSAL by a peer, not committed truth. It may still
change, and it may be wrong. Adapt your own version to fit where that is
right, and say in your report what you changed because of what you saw.
Read once, at finalization — not repeatedly.
-------------------------------------------------------------------------
WARN
  else
    printf 'coedit: no peer diffs for %s\n' "$file"
  fi
  return 0
}

cmd_done() {
  local file="${1:?usage: done <file> <worker>}" worker="${2:?worker required}"
  local d; d="$(dir "$file")"
  [ -e "$d/claims/$worker" ] || die "$worker never claimed $file"
  mkdir -p "$d/done"
  date -u '+%Y-%m-%dT%H:%M:%SZ' > "$d/done/$worker"
  printf 'coedit: %s declared DONE on %s\n' "$worker" "$file"
  cmd_status "$file"
}

cmd_status() {
  local file="${1:?usage: status <file>}"
  local d; d="$(dir "$file")"
  [ -d "$d/claims" ] || { printf 'coedit: no claims on %s\n' "$file"; return 0; }
  local total=0 done_n=0
  for c in "$d/claims"/*; do
    [ -e "$c" ] || continue
    total=$((total+1))
    local w; w="$(basename "$c")"
    if [ -e "$d/done/$w" ]; then done_n=$((done_n+1)); fi
  done
  printf 'coedit: %s — %d/%d claimants done\n' "$file" "$done_n" "$total"
  cmd_peers "$file" ""
  if [ "$total" -gt 0 ] && [ "$done_n" -eq "$total" ]; then
    printf '\nBARRIER OPEN — every claimant finished. The ORCHESTRATOR reconciles.\n'
    printf 'Review each version, then merge. If both intents cannot be preserved,\n'
    printf 'escalate with both visible. Never invent a merge neither worker wrote.\n'
  fi
  return 0
}

cmd_ready() {
  local file="${1:?usage: ready <file>}"
  local d; d="$(dir "$file")"
  [ -d "$d/claims" ] || return 1
  for c in "$d/claims"/*; do
    [ -e "$c" ] || continue
    [ -e "$d/done/$(basename "$c")" ] || return 1
  done
  return 0
}

cmd_list() {
  [ -d "$ROOT" ] || { printf 'coedit: nothing under co-editing\n'; return 0; }
  local any=0
  for d in "$ROOT"/*; do
    [ -d "$d/claims" ] || continue
    any=1
    local file; file="$(basename "$d" | sed 's#__#/#g')"
    local n; n="$(ls -1 "$d/claims" 2>/dev/null | wc -l | tr -d ' ')"
    local k; k="$(ls -1 "$d/done" 2>/dev/null | wc -l | tr -d ' ')"
    printf '  %-56s %s/%s done\n' "$file" "$k" "$n"
  done
  [ "$any" -eq 0 ] && printf 'coedit: nothing under co-editing\n'
  return 0
}

cmd_release() {
  local file="${1:?usage: release <file>}"
  local d; d="$(dir "$file")"
  [ -d "$d" ] || { printf 'coedit: nothing to release for %s\n' "$file"; return 0; }
  rm -rf "$d"
  printf 'coedit: released %s — reconciliation recorded as complete\n' "$file"
}

case "${1:-}" in
  claim)   shift; cmd_claim   "$@" ;;
  peers)   shift; cmd_peers   "$@" ;;
  diff)    shift; cmd_diff    "$@" ;;
  done)    shift; cmd_done    "$@" ;;
  status)  shift; cmd_status  "$@" ;;
  ready)   shift; cmd_ready   "$@" ;;
  list)    shift; cmd_list    "$@" ;;
  release) shift; cmd_release "$@" ;;
  *) sed -n '/^# USAGE/,/^set -uo/p' "$0" | sed 's/^# \{0,1\}//; $d'; exit 2 ;;
esac
