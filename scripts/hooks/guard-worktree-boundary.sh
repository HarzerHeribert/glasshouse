#!/usr/bin/env bash
# PreToolUse guard: a worker may not edit outside the worktree it was given.
#
# WHY THIS EXISTS
# ---------------
# On 2026-08-29 (batch 47) a worker spent thirteen minutes editing the MAIN
# CHECKOUT instead of its own worktree. Six files, +207/-12, including a schema
# migration. Its worktree stayed empty the whole time.
#
# It was not the worker's fault and it was not disobedience. Its packet named
# the packet file and an input report by ABSOLUTE main-checkout path — because
# `.agent-runtime/` is gitignored and therefore does not exist inside a
# worktree — and it simply kept using paths from the same tree it had been
# told to read from. Every packet says "edit only files in your worktree";
# the rule was written down, and being written down did not stop it.
#
# Nothing caught it. It surfaced only because the orchestrator happened to run
# `git status` in the main checkout for an unrelated mutation and saw five
# files it did not recognise. The work was recoverable — captured as a patch,
# replayed into the worktree, diffstat matched the worker's own +207/-12 — but
# the detection was luck, and luck is not a control.
#
# That is the same argument `guard-destructive-git.sh` makes one file over: a
# rule nobody enforces is decoration. This turns "stay in your worktree" into
# something that cannot be forgotten under a packet's own bad example.
#
# WHAT IT BLOCKS
# --------------
# Edit / Write / MultiEdit / NotebookEdit whose target resolves OUTSIDE the
# session's worktree, and only when the session is actually running inside one
# (its cwd is under `<repo>/.worktrees/<name>`). The orchestrator works in the
# main checkout, is not inside `.worktrees/`, and is never restricted.
#
# WHAT IT DOES NOT BLOCK, STATED PLAINLY
# --------------------------------------
# A shell redirect (`cat > /path`, `sed -i`, `tee`) goes through Bash, not
# through an edit tool, and this hook does not see it. Closing that means
# parsing arbitrary shell, which `guard-destructive-git.sh` deliberately does
# not attempt either. The batch-47 worker used the Edit tool, which this
# covers; a determined worker with a heredoc is still on its honour.
#
# Exit 0 allows. Exit 2 blocks and shows stderr to the model.
set -uo pipefail

payload="$(cat)"

read -r tool file cwd <<<"$(printf '%s' "$payload" | /usr/bin/python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("  "); raise SystemExit
ti = d.get("tool_input", {}) or {}
path = ti.get("file_path") or ti.get("notebook_path") or ""
print(d.get("tool_name", ""), path or "-", d.get("cwd", "") or "-")
' 2>/dev/null || printf '  ')"

case "$tool" in
  Edit|Write|MultiEdit|NotebookEdit) ;;
  *) exit 0 ;;
esac
[ "$file" != "-" ] && [ -n "$file" ] || exit 0
[ "$cwd" != "-" ] && [ -n "$cwd" ] || cwd="$PWD"

# Resolve without requiring the target to exist yet: a Write creates new files.
resolve() { /usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1" 2>/dev/null; }

cwd_real="$(resolve "$cwd")"
[ -n "$cwd_real" ] || exit 0

# Am I inside a worker worktree? `<anything>/.worktrees/<name>` is the shape
# CLAUDE.md mandates, and it is the only thing that makes this a worker.
case "$cwd_real" in
  */.worktrees/*) ;;
  *) exit 0 ;;                       # the orchestrator's own checkout
esac
boundary="${cwd_real%%/.worktrees/*}/.worktrees/$(printf '%s' "${cwd_real#*/.worktrees/}" | cut -d/ -f1)"

# An absolute target resolves on its own; a relative one resolves against cwd.
case "$file" in
  /*) target="$(resolve "$file")" ;;
  *)  target="$(resolve "$cwd_real/$file")" ;;
esac
[ -n "$target" ] || exit 0

case "$target" in
  "$boundary"|"$boundary"/*) exit 0 ;;
esac

# THE ONE EXCEPTION, and it is load-bearing: a worker's report.
#
# Every packet's REPORT TO names an absolute path in the MAIN checkout's
# `.agent-runtime/`, because that directory is gitignored and so does not
# exist inside a worktree — and because the orchestrator's watch is armed on
# that path and cannot see a file written anywhere else. Blocking it would
# break the return leg of every worker in the process, which is how a guard
# gets switched off wholesale instead of fixed.
#
# Narrow on purpose: `<repo>/.agent-runtime/<something>.md` and nothing else.
# Not the whole directory (packets, CONTINUATION.md and the evidence drafts
# live there too and are not a worker's to rewrite), and not any other tree.
repo_root="${boundary%%/.worktrees/*}"
case "$target" in
  "$repo_root"/.agent-runtime/*.md)
    case "${target##*/}" in
      report-*) exit 0 ;;
    esac
    ;;
esac

cat >&2 <<EOF
BLOCKED by scripts/hooks/guard-worktree-boundary.sh

  $tool -> $target

That path is OUTSIDE your worktree. You are working in:

  $boundary

and everything you edit must be under it. The file you just named lives in a
different tree — most likely the main checkout, which belongs to the
orchestrator and to every other worker at once.

THE LIKELY CAUSE, AND IT IS NOT YOUR MISTAKE
Your packet names some files by absolute main-checkout path — the packet
itself, and any report it tells you to read — because \`.agent-runtime/\` is
gitignored and does not exist in a worktree. Those paths are for READING ONLY.
Every path you WRITE is relative to your worktree.

WHAT TO DO
  * editing a source file:  use the path relative to your worktree
                            (crates/glasshouse/src/..., not /Users/.../glasshouse/crates/...)
  * writing your report:    that one absolute path is correct and is the single
                            exception — but write it exactly as the packet's
                            REPORT TO line gives it, not a relative
                            \`.agent-runtime/...\` from inside your worktree.

On 2026-08-29 a worker edited the main checkout for thirteen minutes this way,
including a schema migration, while its own worktree stayed empty. It was found
by accident. If you believe you genuinely need to write outside your worktree,
STOP and say so in your report instead.
EOF
exit 2
