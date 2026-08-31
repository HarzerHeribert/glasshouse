#!/usr/bin/env bash
# scripts/stale-workspaces.sh — name every cmux workspace that is provably
# redundant, and (under --watch) nag until it is closed.
#
# WHY THIS EXISTS
# ---------------
# Finished worker panes accumulate silently. On 2026-08-31 nineteen were open
# at once; fifteen had been left by finished batches nobody swept, across two
# orchestrator sessions. Every other watch in this project fires on a worker
# EVENT — a pane that is merely no longer needed produces none, which is why
# it stays open. This is the pipeline.sh of pane hygiene: quiet when clean,
# nagging when not.
#
# A pane is REDUNDANT exactly when its worker is fully retired:
#   - its name matches no live worktree under .worktrees/    (not mid-package)
#   - worker-ack.sh --list does not name it        (nothing awaiting review)
#   - it is not an orchestrator pane, and not a human's pane (worker names
#     never contain spaces; a person's pane title usually does)
# Anything else is KEPT: a quiet pane can be a thinking worker (§28).
# A default-named `Terminal` pane IS flagged — it may be a person's scratch
# shell: the orchestrator reads the pane before closing; this script only
# names, it never closes.
#
# Deliberately NOT consulted: .agent-runtime/{done,dispatched}/ — both keep
# marker files long after a worker is acked and retired (measured 2026-08-31:
# twenty stale done/ markers, five stale dispatched/ ones), so reading them
# would mask exactly the panes this script exists to catch. The worktree and
# the ack queue are the only signals that are live rather than litter.
#
# USAGE
#   scripts/stale-workspaces.sh            # list stale panes + close commands; exit 1 if any
#   scripts/stale-workspaces.sh --watch N  # loop every N seconds; print only when non-empty
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scan() {
  PENDING="$("$REPO/scripts/worker-ack.sh" --list 2>/dev/null | sed -nE 's/^[[:space:]]*-[[:space:]]+([^[:space:]]+).*/\1/p')"
  CMUX_QUIET=1 cmux workspace list 2>/dev/null | while IFS= read -r line; do
    ref=$(printf '%s' "$line" | grep -o 'workspace:[0-9]*' | head -1); [ -n "$ref" ] || continue
    name=$(printf '%s' "$line" | sed -E 's/^[* ]*workspace:[0-9]+[[:space:]]+//; s/[[:space:]]+\[selected\]$//')
    case "$name" in *[Oo]rchestrator*) continue ;; esac
    case "$name" in *' '*) continue ;; esac
    [ -d "$REPO/.worktrees/$name" ] && continue
    if printf '%s\n' "$PENDING" | grep -qx -- "$name"; then continue; fi
    echo "STALE $ref $name — close it: scripts/close-worker.sh $ref $name"
  done
}
if [ "${1:-}" = "--watch" ]; then
  n="${2:-900}"
  while true; do
    out="$(scan || true)"
    [ -n "$out" ] && { echo "stale-workspaces: panes left open with nothing pending (one close-worker.sh call per Bash invocation):"; echo "$out"; }
    sleep "$n"
  done
else
  out="$(scan || true)"
  if [ -n "$out" ]; then echo "$out"; exit 1; else echo "stale-workspaces: none"; fi
fi
