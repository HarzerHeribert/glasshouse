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
#   ~/.cache/glasshouse-worker-targets/<name>
#                           a worker's PERSISTENT build cache (written by
#                           scripts/dev/new-worker.sh's per-worker
#                           .cargo/config.toml). It lives OUTSIDE the
#                           worktree on purpose, so closing or removing a
#                           worktree does NOT remove its cache — that is what
#                           makes a worker's second-ever dispatch under the
#                           same name skip a cold build. Reclaim it
#                           explicitly with --reap-caches once no worktree of
#                           that name is live; plain --clean never touches it.
#
# USAGE
#   scripts/reap-worktrees.sh                # report only
#   scripts/reap-worktrees.sh --clean        # remove build output + ci volumes
#   scripts/reap-worktrees.sh --reap-caches  # remove per-worker build caches
#                                             # with no matching live worktree
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
CLEAN=0; REAP_CACHES=0
for a in "$@"; do
  case "$a" in
    --clean)       CLEAN=1 ;;
    --reap-caches) REAP_CACHES=1 ;;
  esac
done

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

# ---- persistent per-worker build caches (scripts/dev/new-worker.sh) --------
# These live OUTSIDE every worktree by design (see the header), so neither the
# reporting loop above nor plain --clean ever sees or touches them. A cache
# is reclaimable only once no worktree of that exact name is live -- matched
# by NAME, the same key new-worker.sh keys the cache by, not by path, since a
# worktree can be removed and recreated at the same path under a different
# name.
CACHE_ROOT="$HOME/.cache/glasshouse-worker-targets"
echo
printf '%-42s %8s  %s\n' "WORKER CACHE" "size" "STATE"
printf '%s\n' "----------------------------------------------------------------------------------------"
if [ -d "$CACHE_ROOT" ]; then
  live_names="$(git -C "$REPO" worktree list | awk '{print $1}' | while read -r p; do [ -n "$p" ] && basename "$p"; done)"
  cache_total_k=0
  for d in "$CACHE_ROOT"/*/; do
    [ -d "$d" ] || continue
    cname="$(basename "$d")"
    k=$(du -sk "$d" 2>/dev/null | cut -f1)
    human=$(du -sh "$d" 2>/dev/null | cut -f1)
    cache_total_k=$((cache_total_k + k))
    if printf '%s\n' "$live_names" | grep -qxF "$cname"; then
      printf '%-42s %8s  %s\n' "$cname" "$human" "KEEP — worktree $cname is live"
    elif [ "$REAP_CACHES" -eq 1 ]; then
      rm -rf "$d"
      printf '%-42s %8s  removed — no matching live worktree\n' "$cname" "$human"
    else
      printf '%-42s %8s  reclaimable — no matching live worktree\n' "$cname" "$human"
    fi
  done
  printf '\n'
  printf 'persistent worker build caches: %s\n' "$(echo "$cache_total_k" | awk '{printf "%.1f GB", $1/1024/1024}')"
else
  echo "(none — $CACHE_ROOT does not exist yet)"
fi

if [ "$REAP_CACHES" -eq 1 ]; then
  echo "reclaimed caches with no matching live worktree."
else
  echo "Re-run with --reap-caches to reclaim caches with no matching live worktree."
fi
