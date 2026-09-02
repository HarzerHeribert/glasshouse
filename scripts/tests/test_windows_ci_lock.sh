#!/bin/bash
# scripts/tests/test_windows_ci_lock.sh — acceptance tests for the
# scripts/dev/glasshouse-windows-ci mkdir lock and VM-idle check.
#
# Exercises the lock with the VM stubbed: GLASSHOUSE_CI_DRY_RUN=1 replaces
# the scp/ssh upload-and-run sequence with a `sleep 3` and an echo, and
# GLASSHOUSE_CI_DRY_RUN_TASKLIST points at a file standing in for a real
# `tasklist` transcript, so the VM-idle check can be driven without a VM.
# Both stubs are no-ops when unset — the real path is untouched.
set -u

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ci_script="$repo_root/scripts/dev/glasshouse-windows-ci"

pass=0
fail=0

pass_case() { printf 'PASS: %s\n' "$1"; pass=$((pass + 1)); }
fail_case() { printf 'FAIL: %s\n' "$1"; fail=$((fail + 1)); }

check() {
  local desc="$1"
  if [[ "${2:-}" == "0" ]]; then
    pass_case "$desc"
  else
    fail_case "$desc"
  fi
}

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

busy_tasklist="$work_dir/busy_tasklist.txt"
idle_tasklist="$work_dir/idle_tasklist.txt"
printf 'Image Name                     PID Session Name\ncargo.exe                    1234 Console\n' > "$busy_tasklist"
printf 'Image Name                     PID Session Name\nsvchost.exe                     99 Console\n' > "$idle_tasklist"

run_ci() {
  # $1 = cache dir, rest = args to the script. VM is stubbed idle by default
  # so tests that are only exercising the local mkdir lock don't also pay
  # the VM-idle check's 30s sample.
  local cache="$1"
  shift
  XDG_CACHE_HOME="$cache" \
    GLASSHOUSE_CI_DRY_RUN=1 \
    GLASSHOUSE_CI_DRY_RUN_TASKLIST="${GLASSHOUSE_CI_DRY_RUN_TASKLIST:-$idle_tasklist}" \
    GLASSHOUSE_CI_REPO="$repo_root" \
    "$ci_script" "$@"
}

echo "=== two overlapping invocations serialize (REQUIRED BEHAVIOR #1) ==="
cache1="$work_dir/cache1"
mkdir -p "$cache1"
out1="$work_dir/out1.log"
out2="$work_dir/out2.log"

start_ts=$(date +%s)
run_ci "$cache1" build > "$out1" 2>&1 &
pid1=$!
sleep 1 # let the first invocation win the mkdir race
run_ci "$cache1" build > "$out2" 2>&1 &
pid2=$!
wait "$pid1"; status1=$?
wait "$pid2"; status2=$?
end_ts=$(date +%s)
elapsed=$((end_ts - start_ts))

check "both invocations exit 0" "$(( status1 == 0 && status2 == 0 ? 0 : 1 ))"
check "second invocation printed the holder line while waiting" \
  "$(grep -q 'waiting for the VM: held by pid' "$out2" && echo 0 || echo 1)"
check "second invocation ran to completion after waiting" \
  "$(grep -q 'dry-run: would run' "$out2" && echo 0 || echo 1)"
# The second invocation only re-polls every 15s (§ OBJECTIVE #1), so a truly
# serialized pair takes noticeably longer than one dry-run's ~3s body.
check "runs were serialized, not concurrent (elapsed ${elapsed}s >= 10s)" \
  "$(( elapsed >= 10 ? 0 : 1 ))"
if [[ ! -d "$cache1/glasshouse-windows-ci.lock" ]]; then
  pass_case "lock directory released after both runs finished"
else
  fail_case "lock directory released after both runs finished (still present)"
fi

echo "=== a lock held by a dead pid is reclaimed (REQUIRED BEHAVIOR #2a) ==="
cache2="$work_dir/cache2"
mkdir -p "$cache2/glasshouse-windows-ci.lock"
dead_pid=99999
while kill -0 "$dead_pid" 2>/dev/null; do dead_pid=$((dead_pid - 1)); done
printf '%s' "$dead_pid" > "$cache2/glasshouse-windows-ci.lock/pid"
printf 'all, started long ago' > "$cache2/glasshouse-windows-ci.lock/started"

out3="$work_dir/out3.log"
start_ts=$(date +%s)
run_ci "$cache2" build > "$out3" 2>&1
status3=$?
end_ts=$(date +%s)
elapsed3=$((end_ts - start_ts))

check "dead-pid holder run exits 0" "$(( status3 == 0 ? 0 : 1 ))"
check "reclaim was announced" \
  "$(grep -q "Reclaiming stale lock: pid $dead_pid is not running." "$out3" && echo 0 || echo 1)"
# Reclaim itself is instant; the run still pays one VM-idle sample pair
# (~30s) with the default idle stub, so the bound here is against the local
# lock's 15s poll interval, not against zero.
check "reclaim did not wait through a lock poll (elapsed ${elapsed3}s < 40s)" \
  "$(( elapsed3 < 40 ? 0 : 1 ))"

echo "=== a lock held by a live pid is never reclaimed, and NO_WAIT fails fast (REQUIRED BEHAVIOR #2b, #3) ==="
cache3="$work_dir/cache3"
mkdir -p "$cache3/glasshouse-windows-ci.lock"
live_pid=$$
printf '%s' "$live_pid" > "$cache3/glasshouse-windows-ci.lock/pid"
printf 'all, started just now' > "$cache3/glasshouse-windows-ci.lock/started"

out4="$work_dir/out4.log"
start_ts=$(date +%s)
GLASSHOUSE_CI_NO_WAIT=1 run_ci "$cache3" build > "$out4" 2>&1
status4=$?
end_ts=$(date +%s)
elapsed4=$((end_ts - start_ts))

check "GLASSHOUSE_CI_NO_WAIT=1 exits non-zero" "$(( status4 != 0 ? 0 : 1 ))"
check "NO_WAIT failure names the holder" \
  "$(grep -q "waiting for the VM: held by pid $live_pid" "$out4" && echo 0 || echo 1)"
check "NO_WAIT failed immediately, no polling (elapsed ${elapsed4}s < 10s)" \
  "$(( elapsed4 < 10 ? 0 : 1 ))"
check "the live holder's lock was not reclaimed" \
  "$([[ "$(cat "$cache3/glasshouse-windows-ci.lock/pid" 2>/dev/null)" == "$live_pid" ]] && echo 0 || echo 1)"
rm -rf "$cache3/glasshouse-windows-ci.lock"

echo "=== VM-busy: waits, and GLASSHOUSE_CI_NO_WAIT=1 fails fast (ORCHESTRATOR AMENDMENT) ==="
cache4="$work_dir/cache4"
mkdir -p "$cache4"
out5="$work_dir/out5.log"
start_ts=$(date +%s)
GLASSHOUSE_CI_DRY_RUN_TASKLIST="$busy_tasklist" GLASSHOUSE_CI_NO_WAIT=1 \
  run_ci "$cache4" build > "$out5" 2>&1
status5=$?
end_ts=$(date +%s)
elapsed5=$((end_ts - start_ts))

check "busy VM + NO_WAIT exits non-zero" "$(( status5 != 0 ? 0 : 1 ))"
check "busy VM failure names the busy processes" \
  "$(grep -q 'waiting for the VM: cargo.exe or rustc.exe is still running on the VM' "$out5" && echo 0 || echo 1)"
check "busy VM + NO_WAIT failed immediately (elapsed ${elapsed5}s < 10s)" \
  "$(( elapsed5 < 10 ? 0 : 1 ))"

echo "=== VM-idle: two consecutive idle samples let the run proceed ==="
cache5="$work_dir/cache5"
mkdir -p "$cache5"
out6="$work_dir/out6.log"
start_ts=$(date +%s)
GLASSHOUSE_CI_DRY_RUN_TASKLIST="$idle_tasklist" run_ci "$cache5" build > "$out6" 2>&1
status6=$?
end_ts=$(date +%s)
elapsed6=$((end_ts - start_ts))

check "idle VM run exits 0" "$(( status6 == 0 ? 0 : 1 ))"
check "idle VM run reached the dry-run body" \
  "$(grep -q 'dry-run: would run' "$out6" && echo 0 || echo 1)"
# Two samples 30s apart: at least one 30s wait between them.
check "idle VM run took the second-sample wait (elapsed ${elapsed6}s >= 25s)" \
  "$(( elapsed6 >= 25 ? 0 : 1 ))"

echo
echo "=== $pass passed, $fail failed ==="
(( fail == 0 ))
