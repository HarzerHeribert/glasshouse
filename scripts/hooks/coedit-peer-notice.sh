#!/usr/bin/env bash
# PreToolUse advisory: this file has a peer editing it right now.
#
# WHY THIS EXISTS
# ---------------
# `scripts/coedit.sh` lets two workers share a file (practice §77, map Maybe L).
# The protocol depends on each worker *reading the other's version once before
# finalizing* — and a protocol that depends on remembering is a habit, which
# §76 records failing under load in this very project, twice in one round.
#
# So the reminder is delivered by the harness at the moment it matters: the
# instant a worker edits a co-edited file.
#
# WHAT IT DOES, AND WHAT IT DELIBERATELY DOES NOT
# -----------------------------------------------
# It is ADVISORY and NON-BLOCKING. It never refuses an edit. A hook is a gate
# and cannot answer for the model (`docs/process/harness-hook-protocol.md`), so
# it does the one thing a gate does well: it puts text in front of the model.
#
# It does NOT tell the worker what the peer wrote — only that a peer exists and
# how to look. Injecting the peer's diff here would push an unfinished proposal
# into a context that did not ask for it, on every edit, which is the opposite
# of "read once, at finalization".
#
# Silent when the file has no peers, which is almost always.
set -uo pipefail

PAYLOAD="$(cat 2>/dev/null || true)"
REPO="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

read_field() {
  printf '%s' "$PAYLOAD" | python3 -c \
    "import json,sys
try: d=json.load(sys.stdin)
except Exception: print(''); raise SystemExit
v=d.get('tool_input',{}).get('$1','')
print(v if isinstance(v,str) else '')" 2>/dev/null || true
}

TARGET="$(read_field file_path)"
[ -n "$TARGET" ] || exit 0

# Normalise to a repo-relative path, which is how claims are keyed.
case "$TARGET" in
  "$REPO"/*) REL="${TARGET#"$REPO"/}" ;;
  /*)        REL="" ;;                      # outside the repo: not our business
  *)         REL="$TARGET" ;;
esac
[ -n "$REL" ] || exit 0

SLUG="$(printf '%s' "$REL" | sed 's#^\./##; s#[/ ]#__#g')"
CLAIMS="$REPO/.agent-runtime/coedit/$SLUG/claims"
[ -d "$CLAIMS" ] || exit 0

# Who else? A single claimant is this worker itself — not contention.
COUNT="$(ls -1 "$CLAIMS" 2>/dev/null | wc -l | tr -d ' ')"
[ "${COUNT:-0}" -ge 2 ] || exit 0
PEERS="$(ls -1 "$CLAIMS" 2>/dev/null | tr '\n' ' ')"

python3 - "$REL" "$PEERS" <<'PY' 2>/dev/null || exit 0
import json, sys
rel, peers = sys.argv[1], sys.argv[2].strip()
msg = (
    f"CO-EDITED FILE: {rel} is claimed by more than one worker right now "
    f"({peers}).\n"
    "You are NOT blocked and you must not stop. Keep working in your own "
    "worktree as normal.\n"
    "Before you declare this file finished, do exactly two things:\n"
    f"  1. scripts/coedit.sh diff {rel} <your-worker-name>\n"
    "     — read the peer's in-progress version ONCE, and adapt yours to fit "
    "where that is right. It is an unfinished proposal, not committed truth; "
    "it may still change and it may be wrong.\n"
    f"  2. scripts/coedit.sh done {rel} <your-worker-name>\n"
    "     — then say in your report what you changed because of what you saw.\n"
    "Do not edit the peer's worktree. Do not re-read repeatedly: once, at the "
    "end. The orchestrator reconciles when every claimant has declared done."
)
print(json.dumps({
    "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "additionalContext": msg,
    }
}))
PY
exit 0
