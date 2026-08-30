#!/usr/bin/env bash
# Reclaim the disk that finished worker worktrees are still holding.
#
# WHY THIS EXISTS
# ---------------
# Eleven worktrees in one day cost ~55GB of Rust build output and ~42GB of
# per-worktree Linux build volumes, and took a 926GB disk to 99% full. Nobody
# noticed until the machine complained: worktrees pile up silently, one per
# worker, and each one's `target/` is invisible until you go looking.
#
# This reports every glasshouse worktree with the space it is holding, and can
# reclaim the parts that are pure build product.
#
# WHAT IT WILL AND WILL NOT TOUCH
#
#   target/                 removable — gitignored, regenerable, holds no source
#   the ci build volume     removable — a Linux build cache, rebuilt on demand
#   the worktree itself     NEVER — it holds the diff, and until the work is
#                           integrated and pushed that diff is the only copy
#   any tracked file        NEVER
#
# USAGE
#   scripts/reap-worktrees.sh              # report only
#   scripts/reap-worktrees.sh --clean      # remove build output + ci volumes
set -uo pipefail

# REPO here has one job: match `git worktree list`'s own row for the main
# checkout so it is NEVER touched ("never the main checkout" below).
# `git worktree list` itself is already worktree-invariant -- any tree of
# this repo sees the same rows -- so BASH_SOURCE-derived REPO is the only
# variable, and it silently answers about whichever tree the invoked copy
# happens to live in. scripts/ is tracked, so every worktree carries its own
# copy of this script. Reproduced 2026-08-30 (script-tree-audit): run via a
# relative path from a worker's own worktree, the exclusion check missed
# every time -- the main checkout's own row (28G of target/) was listed as a
# reclaimable worktree, and `--clean` run the same way would have deleted the
# main checkout's own build cache, the one thing this script's header
# promises it never touches. git's own worktree metadata names the one real
# main checkout regardless of which copy is running.
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
CLEAN=0
[ "${1:-}" = "--clean" ] && CLEAN=1

printf '%-42s %8s  %-22s %s\n' "WORKTREE" "target/" "UNCOMMITTED" "BRANCH"
printf '%s\n' "----------------------------------------------------------------------------------------"

total_k=0
while read -r path _ branch; do
  [ -z "$path" ] && continue
  [ "$path" = "$REPO" ] && continue          # never the main checkout

  name="$(basename "$path")"
  if [ -d "$path/target" ]; then
    k=$(du -sk "$path/target" 2>/dev/null | cut -f1)
    human=$(du -sh "$path/target" 2>/dev/null | cut -f1)
  else
    k=0; human="-"
  fi
  total_k=$((total_k + k))

  dirty=$(cd "$path" 2>/dev/null && git status --porcelain --untracked-files=all 2>/dev/null | wc -l | tr -d ' ')
  if [ "${dirty:-0}" -gt 0 ]; then
    state="$dirty file(s) — KEEP"
  else
    state="clean"
  fi

  printf '%-42s %8s  %-22s %s\n' "$name" "$human" "$state" "${branch:-}"

  if [ "$CLEAN" -eq 1 ] && [ "$k" -gt 0 ]; then
    rm -rf "$path/target"
    vol="glasshouse-ci-home-$(printf '%s' "$path" | shasum | cut -c1-12)"
    docker volume rm "$vol" >/dev/null 2>&1 && printf '%-42s   removed build volume\n' ""
  fi
done < <(git -C "$REPO" worktree list | awk '{print $1, $2, $3}')

printf '\n'
printf 'build output across worker worktrees: %s\n' "$(echo "$total_k" | awk '{printf "%.1f GB", $1/1024/1024}')"

if [ "$CLEAN" -eq 1 ]; then
  echo "reclaimed. Source and uncommitted work untouched; worktrees kept."
else
  echo
  echo "Nothing removed. Re-run with --clean to reclaim build output and ci volumes."
  echo "UNCOMMITTED marks a worktree whose diff is not yet integrated — its"
  echo "target/ is still safe to remove, but do not delete the worktree itself."
fi
