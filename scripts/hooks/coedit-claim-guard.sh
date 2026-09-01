#!/usr/bin/env bash
# PreToolUse guard: a file your packet declares COEDIT is not editable until
# you have claimed it.
#
# WHY THIS EXISTS
# ---------------
# `scripts/coedit.sh` (practice §77) lets two workers share a file. The whole
# protocol rests on the claim: it is what lets the peer see your version, what
# the Stop hook checks for an open barrier, and what `integrate.sh` names in
# its release nudge. `validate_round.py` enforces that both packets DECLARE
# the co-edit (mutual `COEDIT:` lines), but a declaration in a packet and a
# claim in `.agent-runtime/coedit/` are two different things, and only the
# worker can make the second one. A worker that edits first and claims later
# — or never — has a peer working blind for exactly that long.
#
# `coedit-peer-notice.sh` is the advisory half: it tells you a peer exists.
# This is the gate half: it refuses the edit until the claim exists, because
# the claim takes one second and the edit is the moment it matters.
#
# WHAT IT DOES NOT DO
# -------------------
# It never fires in the main checkout (the orchestrator reconciles, it does
# not claim), never fires on a file the packet does not declare, and never
# reads or injects the peer's version. Silent in almost every edit.
#
# Exit 0 allows. Exit 2 blocks and shows stderr to the model.
set -uo pipefail

PAYLOAD="$(cat 2>/dev/null || true)"

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

# Which tree is this session in? The worker's name is its worktree's basename.
# Kinship and the main checkout come from git-common-dir, never BASH_SOURCE:
# scripts/ is tracked, so every worktree carries a copy of this file and the
# copy that runs is whichever tree the harness resolved $CLAUDE_PROJECT_DIR to.
CWD="$(printf '%s' "$PAYLOAD" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("cwd",""))' 2>/dev/null || true)"
[ -n "$CWD" ] || CWD="$PWD"
TOP="$(git -C "$CWD" rev-parse --show-toplevel 2>/dev/null)" || exit 0
COMMON="$(git -C "$CWD" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || exit 0
MAIN="$(dirname "$COMMON")"
[ "$TOP" != "$MAIN" ] || exit 0               # orchestrator: never restricted
NAME="$(basename "$TOP")"

# The packet that governs this worker. A subcontractor's is a subpacket.
PACKET=""
for cand in "$MAIN/.agent-runtime/packet-$NAME.md" "$MAIN/.agent-runtime/subpacket-$NAME.md"; do
  [ -r "$cand" ] && { PACKET="$cand"; break; }
done
[ -n "$PACKET" ] || exit 0

# Repo-relative path of the edit target, keyed the way claims are.
case "$TARGET" in
  "$TOP"/*) REL="${TARGET#"$TOP"/}" ;;
  /*)       exit 0 ;;                          # outside this worktree: the boundary guard's job
  *)        REL="${TARGET#./}" ;;
esac

# Declared COEDIT in the packet? Same regex as validate_round.py's COEDIT_LINE.
grep -qE "^[[:space:]]*COEDIT:[[:space:]]*\`?${REL}\`?([[:space:]]|$)" "$PACKET" || exit 0

SLUG="$(printf '%s' "$REL" | sed 's#^\./##; s#[/ ]#__#g')"
CLAIM="$MAIN/.agent-runtime/coedit/$SLUG/claims/$NAME"
[ -e "$CLAIM" ] && exit 0

cat >&2 <<EOF
BLOCKED by scripts/hooks/coedit-claim-guard.sh

  $REL is declared COEDIT in your packet ($(basename "$PACKET")) and you have
  not claimed it yet.

The claim is what lets your peer see your version of this file while you
work, and what the orchestrator's release step is keyed on. It takes one
second and must come before the first edit:

    scripts/coedit.sh claim $REL $NAME

Then edit as normal. Before you declare the file finished:

    scripts/coedit.sh diff $REL $NAME     # read the peer's version once
    scripts/coedit.sh done $REL $NAME
EOF
exit 2
