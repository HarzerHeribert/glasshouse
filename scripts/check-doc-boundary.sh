#!/usr/bin/env bash
# Product code may cite product documents. It may never cite process documents.
#
# WHY THIS EXISTS
# ---------------
# This repository holds two kinds of document and they are easy to confuse:
#
#   docs/product/   what Glasshouse IS   — the capability map, design decisions,
#                                          the evidence that a capability works
#   docs/process/   how we BUILD it      — orchestration practice, worker tiers,
#                                          measurements, the handoff, the
#                                          worker-to-worker hook protocol
#
# The confusion is not hypothetical. `GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md` reads
# like a product specification and is a contract between our own worker sessions;
# the orchestrator mis-filed it in a design document once. And on 2026-08-26 this
# check found **four** citations of the orchestration practice file inside shipped
# Rust source.
#
# WHY THAT DIRECTION IS THE ONE THAT MATTERS
# ------------------------------------------
# A product source file citing a process document couples the shipped thing to
# notes about how we ran our agents — notes a future maintainer, or anyone who
# ever reads this code without our transcripts, has no reason to care about and
# no way to act on. If a lesson from the build process genuinely bears on the
# code, **restate it in `docs/product/design-decisions.md` and cite that**. The
# rationale for shipped behaviour belongs in the product's own documents.
#
# The reverse is fine and expected: process documents cite the product freely.
#
# USAGE
#   scripts/check-doc-boundary.sh            # scan, exit 1 on any violation
#   scripts/check-doc-boundary.sh --list     # show what product code cites
set -uo pipefail

ORIG_CWD="$(pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# $REPO above is the SCRIPT's own location, not necessarily the CALLER's
# tree: scripts/ is tracked, so every worktree has its own copy, and this
# check is meant to scan the caller's OWN crates/, not whichever tree the
# invoked copy happens to live in. Reproduced 2026-08-30 (script-tree-audit):
# run via absolute path from a worktree, this scanned the main checkout's
# crates/ instead, with no indication of the mismatch. Same shape and same
# fix as scripts/blast-radius.sh.
common_dir() {
  local d
  d="$(git -C "$1" rev-parse --git-common-dir 2>/dev/null)" || return 1
  case "$d" in
    /*) printf '%s\n' "$d" ;;
    *)  (cd "$1/$d" 2>/dev/null && pwd -P) ;;
  esac
}

CALLER_TOPLEVEL="$(git -C "$ORIG_CWD" rev-parse --show-toplevel 2>/dev/null)"
if [ -n "$CALLER_TOPLEVEL" ]; then
  REPO_TOPLEVEL="$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null)"
  if [ "$CALLER_TOPLEVEL" != "$REPO_TOPLEVEL" ]; then
    REPO_COMMON="$(common_dir "$REPO")"
    CALLER_COMMON="$(common_dir "$CALLER_TOPLEVEL")"
    if [ -n "$REPO_COMMON" ] && [ "$REPO_COMMON" = "$CALLER_COMMON" ]; then
      echo "check-doc-boundary: scanning the caller's worktree at $CALLER_TOPLEVEL (not $REPO)"
      REPO="$CALLER_TOPLEVEL"
    fi
  fi
fi

cd "$REPO" || exit 1

# Source of truth for "a process document", by path and by legacy filename, so
# this keeps working before and after the documents move.
PROCESS_PAT='docs/process/|GLASSHOUSE_ORCHESTRATION_PRACTICE|GLASSHOUSE_ORCHESTRATION_MEASUREMENTS|GLASSHOUSE_WORKER_CAPABILITIES|GLASSHOUSE_ORCHESTRATOR_PROMPT|GLASSHOUSE_AGENT_SDLC|GLASSHOUSE_HARNESS_HOOK_PROTOCOL|GLASSHOUSE_HANDOFF'

if [ "${1:-}" = "--list" ]; then
  echo "documents cited by product source:"
  grep -rhoE 'docs/(product|process)/[a-z/-]*\.md|GLASSHOUSE_[A-Z_]+\.md' \
    --include='*.rs' crates/ 2>/dev/null | sort | uniq -c | sort -rn
  exit 0
fi

hits="$(grep -rnE "$PROCESS_PAT" --include='*.rs' crates/ 2>/dev/null || true)"

if [ -z "$hits" ]; then
  echo "doc boundary: clean — no product source cites a process document."
  exit 0
fi

count="$(printf '%s\n' "$hits" | wc -l | tr -d ' ')"
printf '\033[31mdoc boundary: %s violation(s)\033[0m\n\n' "$count"
printf '%s\n' "$hits" | sed 's/^/  /'
cat <<'MSG'

Product source must not cite a process document. These files describe how
Glasshouse is built, not what it does, and shipped code cannot act on them.

To fix: restate the point in docs/product/design-decisions.md as a decision
about the product, and cite that instead. Delete the process citation.
MSG
exit 1
