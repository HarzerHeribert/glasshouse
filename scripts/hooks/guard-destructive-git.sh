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
# docs/process/orchestration-practice.md §22, and it was broken anyway. That is
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
# Since 2026-09-01 it also blocks the SWEEPING stage — `git add -A`, `git add
# .`, `git add -u` with no pathspec, `git commit -a` — for the opposite
# failure: not losing work, but committing work that was not yours to commit.
# `scripts/integrate.sh` applies a worker's diff into the main checkout and
# deliberately stops there; the orchestrator rules, writes evidence, and
# commits. Between those two steps the tree holds the worker's uncommitted
# implementation, and a `git add -A` written for a one-line docs commit takes
# it along. Commit 645d6cf is titled "correct my own diagnosis" and carries a
# 1005-line routing implementation, pushed, under that message. The
# orchestrator had read the diff minutes earlier. A pathspec forces the
# question "which files is this commit about" at the one moment it is easy to
# answer, so that is the only form allowed.
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

# ---- the sweeping stage: `git add -A|--all|.|:/`, `git add -u` with no
#      pathspec, `git commit -a|--all|-am…`. Tokenised rather than grepped,
#      because `-am` and `--amend` differ by one character and the regex that
#      tells them apart is the kind nobody reads. Python is already a
#      dependency of this hook (the JSON above).
sweep="$(printf '%s' "$command" | /usr/bin/python3 -c '
import shlex, sys, re
cmd = sys.stdin.read()
# Split on the shell operators the flat form can contain. A quoted operator
# inside an argument is rare in a git command and errs toward blocking.
for seg in re.split(r"(?:\|\||&&|;|\||&|\n)", cmd):
    try:
        toks = shlex.split(seg)
    except ValueError:
        toks = seg.split()
    if not toks:
        continue
    # find "git", then skip its own options (-C dir, -c k=v, --git-dir=…)
    try:
        i = toks.index("git")
    except ValueError:
        continue
    i += 1
    while i < len(toks) and toks[i].startswith("-"):
        i += 2 if toks[i] in ("-C", "-c", "--git-dir", "--work-tree") else 1
    if i >= len(toks):
        continue
    sub, args = toks[i], toks[i + 1 :]
    if sub == "add":
        flags = [a for a in args if a.startswith("-") and a != "--"]
        paths = [a for a in args if not a.startswith("-") and a != "--"]
        if "--" in args:
            paths = args[args.index("--") + 1 :]
        if any(f in ("-A", "--all", "--no-ignore-removal") for f in flags) \
           or any(re.fullmatch(r"-[a-zA-Z]*A[a-zA-Z]*", f) for f in flags):
            print("git add --all"); break
        if any(p in (".", "./", ":/", "*", ":/*", "./*") for p in paths):
            print("git add " + [p for p in paths if p in (".", "./", ":/", "*", ":/*", "./*")][0]); break
        if any(f in ("-u", "--update") or re.fullmatch(r"-[a-zA-Z]*u[a-zA-Z]*", f) for f in flags) and not paths:
            print("git add -u with no pathspec"); break
    elif sub == "commit":
        flags = [a for a in args if a.startswith("-")]
        if any(f == "--all" or re.fullmatch(r"-[a-zA-Z]*a[a-zA-Z]*", f) for f in flags):
            print("git commit -a"); break
' 2>/dev/null || true)"

if [ -n "$sweep" ]; then
  status="$(git status --short 2>/dev/null | head -40)"
  cat >&2 <<EOF
BLOCKED by scripts/hooks/guard-destructive-git.sh

  $sweep

This stages EVERYTHING in the tree, and after scripts/integrate.sh the tree
holds a worker's uncommitted implementation next to whatever you meant to
commit. On 2026-09-01 a one-paragraph measurements correction (645d6cf) was
pushed carrying a 1005-line routing implementation the orchestrator had
integrated minutes earlier and was still ruling on.

Name the files. \`git status --short\` right now:

${status:-  (clean)}

Then:

    git add -- <the files this commit is about>
    git commit -m "..."

A commit that cannot name its files is not ready to be made.
EOF
  exit 2
fi

exit 0
