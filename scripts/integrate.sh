#!/usr/bin/env bash
# Apply finished workers' worktree diffs onto the main checkout — the mechanical
# half of integration, and ONLY the mechanical half.
#
# WHY THIS EXISTS, AND WHAT IT DELIBERATELY REFUSES TO DO
# -------------------------------------------------------
# Integration was measured on batch 45: applying six worktrees' diffs, running
# fmt, running the suite, and running the gate. Those steps caught NOTHING on
# their own. Everything the integrator actually caught that round came from
# reading a diff or choosing a mutation — the classify-caller refusal, the four
# mutations a worker's packet had forbidden it to run, a mis-attribution
# diagnosed from a flake report.
#
# The obvious conclusion was "delegate integration to an assistant". That is
# wrong, and the reason matters: **the integrator noticed the classify-caller
# semantics WHILE applying the patch.** An assistant that hands back a green
# tree removes exactly the exposure that produced the ruling, and a green
# summary invites ticking a box without ever reading the code.
#
# The mechanics are deterministic, so a script strictly dominates an agent here:
# cheaper, faster, no context, and no judgment that can be lost. What it will
# not do is anything that requires judgment:
#
#   IT DOES:      check the worktrees are disjoint, verify each base is an
#                 ancestor, apply, copy untracked deliverables, fmt, and run the
#                 blast radius of what changed.
#   IT NEVER:     commits, ticks a box, writes evidence, runs a mutation,
#                 decides whether a diff is acceptable, or reports "ready".
#
# It prints the diffs it applied and stops. Reading them is the integrator's job
# and is not delegable.
#
# USAGE
#   scripts/integrate.sh --dry-run              # plan only: overlap + ancestry
#   scripts/integrate.sh a b c                  # named worktrees under .worktrees/
#   scripts/integrate.sh --all                  # every worktree with changes
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

DRY=0; NAMES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --all)
      for d in "$REPO"/.worktrees/*/; do
        [ -d "$d" ] || continue
        n="$(basename "$d")"
        [ -n "$(git -C "$d" status --porcelain 2>/dev/null)" ] && NAMES+=("$n")
      done ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) NAMES+=("$1") ;;
  esac
  shift
done

[ ${#NAMES[@]} -gt 0 ] || { echo "integrate: name at least one worktree, or --all"; exit 1; }

if [ -n "$(git status --porcelain)" ]; then
  echo "integrate: the main checkout is dirty. Commit or stash first —"
  echo "  applying onto a dirty tree makes 'what did this worker change' unanswerable."
  exit 1
fi

fail=0
printf '\033[1m=== %d worktree(s) ===\033[0m\n' "${#NAMES[@]}"

# ---- 1. ancestry: a diff cut from a base that is not behind HEAD may apply
#         cleanly and still mean something different than the worker intended.
for n in "${NAMES[@]}"; do
  wt="$REPO/.worktrees/$n"
  [ -d "$wt" ] || { echo "  MISSING   $n"; fail=1; continue; }
  base="$(git -C "$wt" rev-parse HEAD 2>/dev/null)"
  if git merge-base --is-ancestor "$base" HEAD 2>/dev/null; then
    printf '  %-22s base %s  ancestor-ok\n' "$n" "$(git rev-parse --short "$base")"
  else
    printf '  %-22s base %s  \033[31mNOT AN ANCESTOR — rebase it first\033[0m\n' \
      "$n" "$(git rev-parse --short "$base")"
    fail=1
  fi
done

# ---- 2. overlap: two workers touching one file is either a co-edit that has
#         been reconciled, or a partition failure. Either way the integrator
#         decides, not this script.
echo
printf '\033[1m=== file overlap ===\033[0m\n'
overlap="$(
  for n in "${NAMES[@]}"; do
    git -C "$REPO/.worktrees/$n" status --porcelain 2>/dev/null | awk -v W="$n" '{print $2, W}'
  done | sort | awk '{f[$1]=f[$1]" "$2} END{for(k in f){n=split(f[k],a," "); if(n>1) print "  " k " ->" f[k]}}'
)"
if [ -n "$overlap" ]; then
  printf '\033[33m%s\033[0m\n' "$overlap"
  echo "  Two workers touched one file. If this is a declared co-edit, reconcile it"
  echo "  yourself (scripts/coedit.sh status <file>) before applying. This script"
  echo "  will not choose a winner."
  fail=1
else
  echo "  none — partitions are disjoint"
fi

[ "$fail" -ne 0 ] && { echo; echo "integrate: refusing to apply, see above"; exit 1; }
[ "$DRY" -eq 1 ] && { echo; echo "integrate: --dry-run, nothing applied"; exit 0; }

# ---- 3. apply, one worktree at a time, stopping at the first failure so a
#         half-integrated tree never happens silently.
echo
for n in "${NAMES[@]}"; do
  wt="$REPO/.worktrees/$n"
  patch="$(mktemp)"
  git -C "$wt" diff HEAD > "$patch"
  if [ -s "$patch" ]; then
    if git apply --check "$patch" 2>/dev/null && git apply "$patch"; then
      printf '  applied  %-22s %s\n' "$n" "$(git -C "$wt" diff --shortstat HEAD)"
    else
      printf '  \033[31mFAILED   %s — patch does not apply\033[0m\n' "$n"
      rm -f "$patch"; exit 1
    fi
  fi
  rm -f "$patch"
  # Untracked deliverables are invisible to `git diff` and are frequently the
  # whole package — a tests-only worker has no tracked changes at all.
  git -C "$wt" ls-files --others --exclude-standard | while read -r f; do
    [ -n "$f" ] || continue
    mkdir -p "$(dirname "$REPO/$f")"
    cp "$wt/$f" "$REPO/$f"
    printf '  copied   %-22s %s\n' "$n" "$f"
  done
done

# ---- 4. fmt is the integrator's, never a worker's (§37), and it must happen
#         before any test so a formatting-only diff never masks a real one.
echo
cargo fmt --all || { echo "integrate: cargo fmt failed"; exit 1; }
echo "  cargo fmt --all: done"

# ---- 5. run what the change could plausibly break, not the whole world.
echo
scripts/blast-radius.sh
rc=$?

echo
printf '\033[1m=== NOW READ THE DIFF. This script has not judged anything. ===\033[0m\n'
git diff --stat
cat <<'NEXT'

Still yours, and not delegable:
  * read every applied diff against what the capability actually promises
  * re-run each worker's decisive mutation (scripts/mutate.sh) — and check the
    "test result:" line names the target that holds the killing test
  * rule on every box, write the evidence, commit, push

NEXT

# ---- 6. co-edit release nudge (packet GH-INTEGRATE-RELEASE-NUDGE): name any
#         barrier a just-integrated worker still holds. Silence is correct
#         when nothing is held — a nudge that always fires is noise, and this
#         script still never releases anything itself: that asserts
#         reconciliation happened, which is a ruling, the same reason it
#         already refuses to commit or tick a box (CLAUDE.md, practice §77).
release_cmds=()
seen_files=""
if [ -x scripts/coedit.sh ]; then
  list_out="$(scripts/coedit.sh list 2>&1)"; list_rc=$?
  if [ "$list_rc" -eq 0 ]; then
    files="$(printf '%s\n' "$list_out" | awk '$1 !~ /^coedit:/ && NF>=2 {print $1}')"
    while IFS= read -r f; do
      [ -n "$f" ] || continue
      peers_out="$(scripts/coedit.sh peers "$f" "" 2>&1)"; peers_rc=$?
      [ "$peers_rc" -eq 0 ] || continue
      for n in "${NAMES[@]}"; do
        printf '%s\n' "$peers_out" | grep -qF "  peer $n " || continue
        case "$seen_files" in
          *"|$f|"*) ;;
          *) seen_files="$seen_files|$f|"; release_cmds+=("$f") ;;
        esac
      done
    done <<COEDIT_FILES
$files
COEDIT_FILES
  else
    echo "integrate: scripts/coedit.sh list failed (exit $list_rc) — skipping the release nudge" >&2
  fi
else
  echo "integrate: scripts/coedit.sh not found — skipping the release nudge" >&2
fi

if [ "${#release_cmds[@]}" -gt 0 ]; then
  echo "One or more just-integrated workers still hold a co-edit barrier (practice §77):"
  for f in "${release_cmds[@]}"; do
    echo "  scripts/coedit.sh release $f"
  done
  cat <<'RECONCILE'
Reconciliation here is not "do the patches agree" — a peer may still be live,
so check whether its outstanding patch still applies to the tree you just
changed before releasing:
  git apply --check <peer worktree diff>
RECONCILE
fi

exit "$rc"
