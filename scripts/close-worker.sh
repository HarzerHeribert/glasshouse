#!/usr/bin/env bash
# Close a finished worker's cmux workspace WITHOUT losing its context or
# leaving its processes behind.
#
# WHY THIS EXISTS
# ---------------
# Two things go wrong when a worker pane is closed by hand, and both were paid
# for on 2026-08-26.
#
#  1. **The conversation is discarded.** A finished worker holds an hour of
#     reading and reasoning. cmux already knows how to restart it — `cmux
#     surface resume get` prints the exact command, session id included — but
#     that command dies with the workspace unless somebody writes it down.
#
#  2. **The process outlives the pane.** Four `glasshouse` processes were found
#     spinning at ~99% CPU, three of them nineteen hours old, orphaned to
#     launchd by panes that had been closed. Glasshouse could not see them and
#     would have started more beside them.
#
# Phase 10A makes (2) the product's job: adopt, verify, quarantine. **This
# script is the interim hook that fires until then**, and it deliberately
# behaves the way that phase's fixed requirements say Glasshouse must —
# it REPORTS and REFUSES, it never reaps. Ending someone's session is theirs
# to decide.
#
# NEVER run this on the orchestrator's own workspace. A `caffeinate -w <pid>`
# is holding the machine awake against that session; closing it lets the
# MacBook sleep and stops the fleet.
#
# USAGE
#   scripts/close-worker.sh <workspace-ref> <name> [worktree-path]
#   scripts/close-worker.sh --scan            # just look for orphans
set -uo pipefail

# .agent-runtime/resume is a single project-wide log the orchestrator relies
# on to reopen a closed worker's conversation, and .worktrees/<name> is
# addressed relative to the ONE main checkout. scripts/ is tracked, so every
# worktree carries its own copy of this script -- deriving REPO from
# BASH_SOURCE alone silently redirects both to whichever tree the invoked
# copy happens to live in. Reproduced 2026-08-30 (script-tree-audit): run via
# a relative path from a worker's own worktree, this wrote the captured
# resume commands into that worktree's own (throwaway) .agent-runtime,
# reporting success, while the main checkout -- where the orchestrator would
# ever look -- never saw them. Same fix as scripts/ask-user.sh.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAIN_COMMON="$(git -C "$SCRIPT_DIR" rev-parse --git-common-dir 2>/dev/null)"
case "$MAIN_COMMON" in
  /*) : ;;
  *)  MAIN_COMMON="$(cd "$SCRIPT_DIR/$MAIN_COMMON" 2>/dev/null && pwd -P)" ;;
esac
if [ -n "$MAIN_COMMON" ] && [ "$(basename "$MAIN_COMMON")" = ".git" ]; then
  REPO="$(dirname "$MAIN_COMMON")"
else
  REPO="$SCRIPT_DIR"
fi
RESUME_DIR="$REPO/.agent-runtime/resume"

scan_orphans() {
  printf '\n\033[1m=== processes that may have outlived a pane ===\033[0m\n'
  local found=0
  # A build of this project, or anything named glasshouse, burning CPU.
  while read -r pid cpu etime cmd; do
    case "$cpu" in ''|*[!0-9.]*) continue;; esac
    # shellcheck disable=SC2072
    if awk "BEGIN{exit !($cpu > 20)}"; then
      found=1
      printf '  pid %-7s %5s%%  up %-12s %s\n' "$pid" "$cpu" "$etime" "${cmd:0:70}"
    fi
  done < <(ps -axo pid=,pcpu=,etime=,command= | grep -i '[g]lasshouse' | grep -v 'close-worker\|worker-watch\|claude ')

  if [ "$found" -eq 0 ]; then
    printf '  none\n'
    return 0
  fi
  printf '\n\033[33mNOT killed.\033[0m Phase 10A quarantines rather than reaps, and so does this.\n'
  printf 'If these are yours and finished:  kill -TERM <pid>\n'
  printf 'Known cause: the TUI spins when its terminal dies — see\n'
  printf '  .agent-runtime/defect-tui-spins-when-terminal-dies.md\n'
}

if [ "${1:-}" = "--scan" ]; then
  scan_orphans
  exit 0
fi

WS="${1:?usage: close-worker.sh <workspace-ref> <name>   |   --scan}"
NAME="${2:?missing worker name}"

# Refuse to close the session holding the machine awake.
SELF_WS="$(CMUX_QUIET=1 cmux identify 2>/dev/null | sed -n 's/.*"workspace_ref" : "\([^"]*\)".*/\1/p' | head -1)"
if [ -n "$SELF_WS" ] && [ "$WS" = "$SELF_WS" ]; then
  echo "refusing: $WS is this orchestrator's own workspace." >&2
  echo "A caffeinate is holding the machine awake against it; closing it stops the fleet." >&2
  exit 2
fi

mkdir -p "$RESUME_DIR"
OUT="$RESUME_DIR/$NAME.txt"

# 1. Keep the address of the conversation before the workspace takes it away.
{
  echo "# worker: $NAME"
  echo "# workspace: $WS"
  echo "# captured: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "#"
  echo "# Reopen this worker's full context by running the line below."
  echo "# It is cmux's own restart command for the surface, not a reconstruction."
  echo
} > "$OUT"

surfaces="$(CMUX_QUIET=1 cmux list-pane-surfaces --workspace "$WS" 2>/dev/null \
            | grep -oE 'surface:[0-9]+' || true)"
if [ -z "$surfaces" ]; then
  echo "warning: no surfaces found in $WS; nothing to capture" >&2
else
  for s in $surfaces; do
    echo "## $s" >> "$OUT"
    CMUX_QUIET=1 cmux surface resume get --surface "$s" >> "$OUT" 2>/dev/null \
      || echo "(no resume command recorded)" >> "$OUT"
    echo >> "$OUT"
  done
  echo "kept resume commands for $NAME -> $OUT"
fi

# 2. Now the workspace may go.
CMUX_QUIET=1 cmux close-workspace --workspace "$WS" >/dev/null 2>&1 \
  && echo "closed $WS" \
  || { echo "failed to close $WS" >&2; exit 1; }

# 3. Reclaim the disk this worker was holding.
#
# Not a tidiness step. Eleven worktrees in one day cost ~55GB of Rust build
# output plus ~42GB of per-worktree Linux build volumes, and took a 926GB disk
# to 99% full. Both are pure build product: gitignored, regenerable, and holding
# nothing a diff needs.
#
# **The per-worktree volume is deliberate and stays deliberate.** One shared
# volume was tried and produced a build of `main` that compiled a test file from
# another branch — a shared *source* tree is a wrong-green waiting to happen, and
# sharing a `target/` between two trees is the same hazard one layer down. The
# answer is not to share it; it is to delete it when the worker is done.
WT="${3:-$REPO/.worktrees/$NAME}"
[ -e "$WT" ] || [ ! -e "$REPO/../glasshouse-$NAME" ] || WT="$REPO/../glasshouse-$NAME"
if [ -d "$WT/.git" ] || [ -f "$WT/.git" ]; then
  if [ -d "$WT/target" ]; then
    sz="$(du -sh "$WT/target" 2>/dev/null | cut -f1)"
    rm -rf "$WT/target" && echo "reclaimed $sz of build output from $NAME's worktree"
  fi
  VOL="glasshouse-ci-home-$(cd "$WT" 2>/dev/null && printf '%s' "$PWD" | shasum | cut -c1-12)"
  if docker volume rm "$VOL" >/dev/null 2>&1; then
    echo "removed this worktree's Linux build volume ($VOL)"
  fi
else
  echo "note: no worktree found at $WT; skipped build-output cleanup"
fi

# The worktree itself is NOT removed. It holds the diff, and until the
# orchestrator has integrated and pushed, that diff is the only copy.
echo "worktree kept at $WT — remove it yourself once the work is pushed:"
echo "    git worktree remove $WT"

# 4. And check what it left behind.
scan_orphans
