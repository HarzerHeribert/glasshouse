#!/usr/bin/env bash
# scripts/swarm-collate.sh — read every swarm finding into one index, no
# cross-contamination: each agent owns exactly one .md under break/ or
# quality/, and each proposed fix is a .patch under patches/. This only READS.
set -euo pipefail
SD="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.agent-runtime/swarm-2026-08-31"
[ -d "$SD" ] || { echo "no swarm dir"; exit 0; }
for lane in break quality; do
  echo "========================================================  $lane"
  for f in "$SD/$lane"/*.md; do
    [ -e "$f" ] || { echo "  (none yet)"; continue; }
    n=$(grep -c '^### ' "$f" 2>/dev/null || echo 0)
    printf '  %-34s  %s findings\n' "$(basename "$f")" "$n"
    grep -E '^### |^SEVERITY:|^FIX:' "$f" 2>/dev/null | sed 's/^/      /' | cut -c1-160 || true
  done
done
echo "========================================================  patches"
ls -1 "$SD/patches" 2>/dev/null | sed 's/^/  /' || echo "  (none)"
echo "========================================================  ledger"
cat "$SD/ACCEPTANCE.md" 2>/dev/null || echo "  (no rulings yet)"
