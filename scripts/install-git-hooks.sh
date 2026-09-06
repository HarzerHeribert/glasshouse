#!/usr/bin/env bash
# Point this repository's core.hooksPath at scripts/git-hooks. Idempotent.
# --check exits 1 with a one-line hint when it is not already set.
set -uo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "install-git-hooks.sh: not a git repository" >&2
  exit 1
}

if [ "${1:-}" = "--check" ]; then
  current="$(git config --get core.hooksPath 2>/dev/null || true)"
  if [ "$current" = "scripts/git-hooks" ]; then
    exit 0
  fi
  echo "core.hooksPath is not set to scripts/git-hooks -- run scripts/install-git-hooks.sh" >&2
  exit 1
fi

git -C "$repo_root" config core.hooksPath scripts/git-hooks
echo "core.hooksPath set to scripts/git-hooks (in $repo_root)"
