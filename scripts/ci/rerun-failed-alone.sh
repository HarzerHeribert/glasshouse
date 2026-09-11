#!/usr/bin/env bash
# Re-run every test that failed in a `cargo test --no-fail-fast -- --nocapture`
# log, alone and once, and answer whether any is still red.
#
# WHY. A load-sensitive pty test on a slow runner turns a whole cell red, and
# because a red step stops the job it hides the clippy and rustdoc reds behind
# it (user, 2026-09-11: "tests like these stop the whole test chain from
# running -- meaning we can't catch what's beyond that"). CLAUDE.md rule 4
# already gives such a test one rerun alone in the local gate; this is the
# same rule for the sweep, per failed TEST rather than per family: a
# deterministic failure cannot pass alone, and a test that passes alone but
# fails beside its neighbours is an isolation defect that is NAMED below as
# `flaky-pass`, never hidden.
#
# USAGE: rerun-failed-alone.sh <test-log> [wrapper-command...]
#   <test-log> is the captured stdout of the cargo test run. Each failed test
#   is rerun by executing its test binary directly -- the path cargo printed
#   on its `Running ... (target/...)` line -- with `<name> --exact
#   --test-threads=1`, so no crate name has to be recovered from the log. A
#   wrapper command, when given, is put in front of every rerun (the Linux
#   cell's Secret Service fixture).
#
# EXIT 0 when nothing failed or every failure passed alone (each one printed
# as `flaky-pass:`); exit 1 when any test is red alone (`still red:`, with
# its panic quoted). One rerun, never a third: three flaky-passes of one test
# in a week buy a determinism packet, not another loop.
set -uo pipefail

log="${1:?usage: rerun-failed-alone.sh <test-log> [wrapper-command...]}"
shift

failed="$(awk '
  /^ *Running .*\(target[\/\\][^)]*\)/ {
    match($0, /\(target[\/\\][^)]*\)/)
    bin = substr($0, RSTART + 1, RLENGTH - 2)
  }
  /^ *Doc-tests / { bin = "" }
  /^test .* \.\.\. FAILED$/ {
    name = $0
    sub(/^test /, "", name)
    sub(/ \.\.\. FAILED$/, "", name)
    if (bin != "") print bin "\t" name
  }
' "$log" | sort -u)"

if [ -z "$failed" ]; then
  echo "rerun-failed-alone: no failed test with a runnable binary in $log"
  exit 0
fi

still_red=0
flaky=0
while IFS=$'\t' read -r bin name; do
  [ -n "$bin" ] || continue
  echo "rerun-failed-alone: $name ($bin) -- alone, once"
  if "$@" "$bin" "$name" --exact --test-threads=1 --nocapture > "$log.rerun" 2>&1; then
    flaky=$((flaky + 1))
    printf 'flaky-pass: %s (%s) -- red beside its neighbours, green alone: load-sensitive, not a defect in what it tests\n' "$name" "$bin"
  else
    still_red=$((still_red + 1))
    printf 'still red: %s (%s) -- red alone too: a real failure\n' "$name" "$bin"
    grep -a -A8 'panicked at' "$log.rerun" | head -30
  fi
done <<< "$failed"

echo "rerun-failed-alone: $flaky flaky-pass, $still_red still red"
[ "$still_red" -eq 0 ]
