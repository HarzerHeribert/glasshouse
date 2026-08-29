#!/usr/bin/env bash
# Run the tests of every file a change could plausibly break.
#
# WHY THIS EXISTS
# ---------------
# Practice §69 said: after a change, grep for the symbols it touches to find the
# blast radius. Batch 45 proved that is not enough. `codex-hooks` added two
# entries to `REPORTED_EVENTS` and ran exactly that grep. The grep WORKED — it
# named `session/select.rs`. The worker then *read* that file, judged it
# unaffected, and reported. It was wrong: `codex_hooks_are_written_where_codex_
# reads_them` hardcodes the event list, and the gate failed on macOS AND Linux,
# costing a full gate cycle for something `cargo test --lib session::select`
# would have caught in eight seconds.
#
# The test name contains neither `REPORTED_EVENTS` nor `event`, so no smarter
# grep would have helped. The failing step was a human reading a file to decide
# whether it mattered. §79's rule:
#
#     Once a blast-radius grep names a file, RUN that file's tests.
#     Do not read them and judge.
#
# A written rule already failed this way once — §75 was violated again ninety
# minutes later by its own author, which is why the cited-seams check became
# code. Same treatment here.
#
# USAGE
#   scripts/blast-radius.sh                 # changed vs HEAD (default)
#   scripts/blast-radius.sh --staged        # staged changes
#   scripts/blast-radius.sh --since <ref>   # changed since a ref
#   scripts/blast-radius.sh --dry-run       # print the plan, run nothing
#   scripts/blast-radius.sh f1.rs f2.rs     # explicit files
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

DRY=0; MODE="head"; SINCE=""; FILES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --staged)  MODE="staged" ;;
    --since)   MODE="since"; SINCE="${2:-}"; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *)         FILES+=("$1") ;;
  esac
  shift
done

if [ ${#FILES[@]} -eq 0 ]; then
  case "$MODE" in
    staged) mapfile -t FILES < <(git diff --cached --name-only -- '*.rs') ;;
    since)  mapfile -t FILES < <(git diff --name-only "$SINCE" -- '*.rs') ;;
    *)      mapfile -t FILES < <({ git diff --name-only -- '*.rs'
                                   git diff --cached --name-only -- '*.rs'
                                   git ls-files --others --exclude-standard -- '*.rs'; } | sort -u) ;;
  esac
fi

if [ ${#FILES[@]} -eq 0 ]; then
  echo "blast-radius: no changed .rs files — nothing to do"
  exit 0
fi

printf '\033[1m=== changed (%d) ===\033[0m\n' "${#FILES[@]}"
printf '  %s\n' "${FILES[@]}"

# ---- 1. symbols the changed files DEFINE -----------------------------------
# Only definitions: a file that merely *uses* a symbol is already in the list.
# Deliberately generous about kinds (const/static/fn/struct/enum/trait/type) and
# deliberately silent about visibility — a `pub(crate)` constant is exactly what
# bit us, and filtering to `pub` would have missed it.
SYMS_FILE="$(mktemp)"; trap 'rm -f "$SYMS_FILE" "${HITS_FILE:-}"' EXIT
for f in "${FILES[@]}"; do
  [ -f "$f" ] || continue
  grep -hoE '^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?(const|static|fn|struct|enum|trait|type)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$f" 2>/dev/null \
    | awk '{print $NF}'
done | sort -u | grep -vE '^(new|main|default|fmt|from|drop|clone|next|len|get|set|run|open|read|write)$' > "$SYMS_FILE"

echo
printf '\033[1m=== %d defined symbol(s) to trace ===\033[0m\n' "$(wc -l < "$SYMS_FILE" | tr -d ' ')"

# ---- 2. files that REFERENCE those symbols ---------------------------------
# A symbol referenced by dozens of files is not a blast radius, it is a common
# word ("Event", "Handler", "Config"). Tracing it drags in the whole crate and
# the signal drowns — the first version of this script traced 32 symbols from
# one file into 80 modules, which is the same as running everything.
#
# So each symbol is capped by fan-out: keep the ones that name a specific edge,
# drop the ones that name a concept. REPORTED_EVENTS (the constant that actually
# bit us) reaches four files and survives; `new` reaches hundreds and does not.
MAX_FANOUT=${BLAST_MAX_FANOUT:-12}
HITS_FILE="$(mktemp)"
printf '%s\n' "${FILES[@]}" > "$HITS_FILE"
kept=0; dropped=0
while read -r sym; do
  [ -n "$sym" ] || continue
  mapfile -t refs < <(grep -rlE "\b${sym}\b" crates/*/src crates/*/tests 2>/dev/null)
  n=${#refs[@]}
  if [ "$n" -gt 0 ] && [ "$n" -le "$MAX_FANOUT" ]; then
    printf '%s\n' "${refs[@]}" >> "$HITS_FILE"
    kept=$((kept+1))
  else
    dropped=$((dropped+1))
  fi
done < "$SYMS_FILE"
sort -u "$HITS_FILE" -o "$HITS_FILE"
printf '  %d symbol(s) kept, %d dropped as too generic (fan-out > %d)\n' \
  "$kept" "$dropped" "$MAX_FANOUT"

# ---- 3. map files -> cargo test targets ------------------------------------
# crates/<pkg>/tests/<name>.rs  -> --test <name>
# crates/<pkg>/src/main.rs      -> --bin <pkg>
# crates/<pkg>/src/**.rs        -> --lib, filtered by module path
LIB=0; declare -a TESTS=() BINS=() FILTERS=()
while read -r hit; do
  [ -n "$hit" ] || continue
  case "$hit" in
    crates/*/tests/*.rs) TESTS+=("$(basename "$hit" .rs)") ;;
    crates/*/src/main.rs) BINS+=("$(echo "$hit" | cut -d/ -f2)") ;;
    crates/*/src/*.rs|crates/*/src/**/*.rs)
      LIB=1
      # src/gateway/session.rs -> gateway::session ; src/foo.rs -> foo
      m="$(echo "$hit" | sed -E 's#^crates/[^/]+/src/##; s#\.rs$##; s#/mod$##; s#/#::#g')"
      [ "$m" = "lib" ] || FILTERS+=("$m")
      ;;
  esac
done < "$HITS_FILE"

# de-dup
mapfile -t TESTS < <(printf '%s\n' "${TESTS[@]-}" | sort -u | sed '/^$/d')
mapfile -t BINS  < <(printf '%s\n' "${BINS[@]-}"  | sort -u | sed '/^$/d')
mapfile -t FILTERS < <(printf '%s\n' "${FILTERS[@]-}" | sort -u | sed '/^$/d')

echo
printf '\033[1m=== plan ===\033[0m\n'
[ "$LIB" -eq 1 ] && echo "  --lib  (module filters: ${FILTERS[*]-none})"
[ ${#TESTS[@]} -gt 0 ] && echo "  --test ${TESTS[*]}"
[ ${#BINS[@]}  -gt 0 ] && echo "  --bin  ${BINS[*]}"

if [ "$DRY" -eq 1 ]; then echo; echo "blast-radius: --dry-run, nothing executed"; exit 0; fi

# ---- 4. run ----------------------------------------------------------------
# One cargo invocation where possible; test-name filters are applied per module
# so a huge --lib run does not swamp the signal (§68: a filter matching nothing
# looks exactly like a pass, so the count is printed either way).
rc=0
run_target() {                     # run_target <label> <cargo args...>
  local label="$1"; shift
  echo; printf '\033[1m=== %s ===\033[0m\n' "$label"
  local out; out="$(mktemp)"
  cargo test -p glasshouse --all-features "$@" >"$out" 2>&1
  local status=$?
  # §68: a filter that matches nothing looks exactly like a pass, so always show
  # the result line, including the count.
  grep -E 'test result:|^error' "$out" | tail -6
  grep -q 'test result: FAILED' "$out" && status=1
  [ "$status" -ne 0 ] && { echo "  --- failures ---"; grep -A4 '^failures:' "$out" | head -12; }
  rm -f "$out"
  return "$status"
}

if [ "$LIB" -eq 1 ]; then
  run_target "cargo test --lib" --lib || rc=1
fi
for t in "${TESTS[@]-}"; do
  [ -n "$t" ] || continue
  run_target "cargo test --test $t" --test "$t" || rc=1
done
for b in "${BINS[@]-}"; do
  [ -n "$b" ] || continue
  run_target "cargo test --bin $b" --bin "$b" || rc=1
done

echo
if [ "$rc" -eq 0 ]; then
  printf '\033[32mblast-radius: every traced target passed\033[0m\n'
else
  printf '\033[31mblast-radius: FAILURES above — fix before the gate\033[0m\n'
fi
exit "$rc"
