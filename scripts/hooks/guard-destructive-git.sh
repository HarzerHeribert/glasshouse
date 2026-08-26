#!/usr/bin/env bash
# PreToolUse guard: refuse the git commands that silently delete uncommitted work.
#
# WHY THIS EXISTS
# ---------------
# On 2026-08-26 the orchestrator ran
#
#     git checkout -- crates/glasshouse/src/provider/mod.rs
#
# inside a finished worker's worktree, to undo a small probe it had appended to
# that file. It deleted 161 lines of the worker's work. Workers never commit —
# their deliverable exists *only* as uncommitted changes — so to git there is no
# difference between the worker's edits and yours, and the undo takes both.
#
# The rule was already written down, in the orchestrator's memory and in
# GLASSHOUSE_ORCHESTRATION_PRACTICE.md §22, and it was broken anyway. That is
# the whole argument for this file: a rule nobody enforces is decoration, which
# is the same finding that two dead CI gates produced the same morning. This
# turns the rule into something that cannot be forgotten under time pressure.
#
# WHAT IT BLOCKS
# --------------
# `git checkout` with a path, `git restore`, `git stash`, and `git clean` — the
# four ways to discard uncommitted changes. Branch operations (`checkout -b`,
# `checkout <branch>`, `switch`) are untouched, because they do not destroy
# working-tree edits.
#
# THE REPLACEMENT, WHICH IS ALWAYS AVAILABLE
# ------------------------------------------
#     cp file /tmp/file.bak     # before
#     …edit or mutate…
#     cp /tmp/file.bak file     # after
#     touch file                # so cargo rebuilds (practice §16)
#
# A copy restores *your* change. A checkout restores *the file*.
#
# Exit 0 allows. Exit 2 blocks and shows stderr to the model.
set -uo pipefail

payload="$(cat)"

tool="$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys;print(json.load(sys.stdin).get("tool_name",""))' 2>/dev/null || true)"
[ "$tool" = "Bash" ] || exit 0

command="$(printf '%s' "$payload" | /usr/bin/python3 -c \
  'import json,sys;print(json.load(sys.stdin).get("tool_input",{}).get("command",""))' 2>/dev/null || true)"
[ -n "$command" ] || exit 0

# Normalise whitespace so `git   checkout` and a multi-line command both match.
flat="$(printf '%s' "$command" | tr '\n' ' ' | tr -s ' ')"

deny() {
  cat >&2 <<EOF
BLOCKED by scripts/hooks/guard-destructive-git.sh

  $1

This discards uncommitted changes, and in a worker's worktree the uncommitted
changes ARE the deliverable — workers never commit. git cannot tell your edit
from theirs, so this reverts the file, not your change to it.

On 2026-08-26 exactly this deleted 161 lines of a finished worker's work.

Use a copy instead, which restores your change rather than the file:

    cp <file> /tmp/<file>.bak
    …edit or mutate…
    cp /tmp/<file>.bak <file>
    touch <file>

If you genuinely need to discard someone's uncommitted work, ask the user
first — that is their call, not yours.
EOF
  exit 2
}

# `git checkout` naming a path. Branch switching and creation are fine.
if printf '%s' "$flat" | grep -qE '(^|[;&|(] *)git +(-{1,2}[A-Za-z][^ ]* +([^-][^ ]* +)?)*checkout\b'; then
  if printf '%s' "$flat" | grep -qE 'checkout +(-[a-zA-Z-]+ +)*--( |$)' \
    || printf '%s' "$flat" | grep -qE 'checkout +(-[a-zA-Z-]+ +)*[^ -][^ ]*/[^ ]*'; then
    printf '%s' "$flat" | grep -qE 'checkout +(-b|-B)\b' || deny "git checkout with a path"
  fi
fi

printf '%s' "$flat" | grep -qE '(^|[;&|(] *)git +(-{1,2}[A-Za-z][^ ]* +([^-][^ ]* +)?)*restore\b' && deny "git restore"

# `git stash` with no subcommand, or push/save, discards the working tree; drop
# and clear destroy saved entries. `list` and `show` only read, and blocking a
# read-only command is how a guard teaches people to route around it.
if printf '%s' "$flat" | grep -qE '(^|[;&|(] *)git +(-{1,2}[A-Za-z][^ ]* +([^-][^ ]* +)?)*stash\b'; then
  printf '%s' "$flat" | grep -qE 'stash +(list|show)\b' || deny "git stash"
fi

# `git clean -n` / `--dry-run` only reports what would go.
if printf '%s' "$flat" | grep -qE '(^|[;&|(] *)git +(-{1,2}[A-Za-z][^ ]* +([^-][^ ]* +)?)*clean\b'; then
  printf '%s' "$flat" | grep -qE 'clean +([^ ]+ +)*(-[a-zA-Z]*n[a-zA-Z]*|--dry-run)\b' || deny "git clean"
fi

exit 0
