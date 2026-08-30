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

ORIG_CWD="$(pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Resolve which tree to analyse. $REPO is the SCRIPT's own location, not
# necessarily the CALLER's tree: every editing worker runs from a worktree
# under .worktrees/, and this script is reachable by absolute path / PATH
# from the main checkout. Everything below this point is cwd-relative (git
# diff, the changed-file existence checks, the crates/*/src grep, the cargo
# invocations) -- so cd'ing unconditionally to $REPO makes the rest of the
# script silently diff and test the WRONG tree. See the header comment for
# the incident this guards against.
#
# Kinship is git-common-dir, not a path prefix: a worktree of this repo
# shares one git dir with the main checkout no matter where it lives, and a
# prefix check would break the moment a worktree lives outside .worktrees/
# while looking correct for the common case.
common_dir() {                     # absolute common .git dir for tree "$1"
  local d
  d="$(git -C "$1" rev-parse --git-common-dir 2>/dev/null)" || return 1
  case "$d" in
    # git itself reports the common dir through a resolved (physical) path
    # when it is already absolute; `pwd -P` matches that for the relative
    # case so the two forms compare equal instead of differing by a
    # symlinked tmp/mount prefix (e.g. macOS /var vs /private/var).
    /*) printf '%s\n' "$d" ;;
    *)  (cd "$1/$d" 2>/dev/null && pwd -P) ;;
  esac
}

CALLER_TOPLEVEL="$(git -C "$ORIG_CWD" rev-parse --show-toplevel 2>/dev/null)"

if [ -z "$CALLER_TOPLEVEL" ]; then
  echo "blast-radius: refusing -- '$ORIG_CWD' is not a git worktree (script lives at '$REPO')" >&2
  exit 1
fi

# Compare through git's own (symlink-resolved) view of $REPO, not the logical
# BASH_SOURCE-derived path, so a caller reached through a symlinked mount does
# not spuriously look like "a different tree" and print a line case 1 must
# never print.
REPO_TOPLEVEL="$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null)"

if [ "$CALLER_TOPLEVEL" != "$REPO_TOPLEVEL" ]; then
  REPO_COMMON="$(common_dir "$REPO")"
  CALLER_COMMON="$(common_dir "$CALLER_TOPLEVEL")"
  if [ -z "$REPO_COMMON" ] || [ "$REPO_COMMON" != "$CALLER_COMMON" ]; then
    echo "blast-radius: refusing -- '$CALLER_TOPLEVEL' is not a worktree of the repo at '$REPO'" >&2
    exit 1
  fi
  echo "blast-radius: analysing the caller's worktree at $CALLER_TOPLEVEL (not $REPO)"
  REPO="$CALLER_TOPLEVEL"
fi

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

# Platform-conditional code: warn that this tool is macOS-only evidence.
#
# WHY: on 2026-08-30 the full gate returned 13 PASS / 3 FAIL on a tree this
# script had called green. All three failures were in `pty/mod.rs`'s
# platform-conditional code — a Windows build error (`-D warnings` on an unused
# constant that is `None` there) and a Linux test failure (a hazard that exists
# on macOS/BSD and *not* on Linux, so a ceiling derived from the documented
# constant rather than measured was protecting against nothing).
#
# This script runs on one platform. That is not a defect in it — but it means
# "blast radius green" is never evidence about the other two, and nineteen
# commits accumulated before anyone ran the gate that could see them. So say so,
# loudly, exactly when it matters.
if [ "${#FILES[@]}" -gt 0 ]; then
  plat=""
  for f in "${FILES[@]}"; do
    [ -f "$f" ] || continue
    case "$f" in *.rs)
      if grep -qE '#\[cfg\((target_os|unix|windows)|cfg!\((unix|windows|target_os)' "$f" 2>/dev/null; then
        plat="$plat $f"
      fi ;;
    esac
  done
  if [ -n "$plat" ]; then
    printf '\n\033[33m=== PLATFORM-CONDITIONAL CODE CHANGED ===\033[0m\n'
    printf '  These files contain cfg(unix/windows/target_os):\n'
    for f in $plat; do printf '    %s\n' "$f"; done
    printf '  \033[33mThis script runs on THIS platform only. A green result here is\n'
    printf '  NOT evidence about the other two.\033[0m Run the full gate before\n'
    printf '  believing it:  scripts/ci-local.sh --macos --linux --windows-vm\n'
  fi
fi

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
  # Show the panic MESSAGE, not only the failing test's name. cargo prints the
  # message under `---- <test> stdout ----`, which is ABOVE `failures:`, so the
  # `-A4 '^failures:'` grep below never reaches it. Measured 2026-08-30: a test
  # whose whole purpose was a self-diagnosing panic ("exited=Some(_) means the
  # signal killed the shell; None means the trap is merely late") failed here
  # and this script printed its file:line and threw the diagnosis away.
  [ "$status" -ne 0 ] && { echo "  --- why ---"; grep -A6 'panicked at' "$out" | head -24; }
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

# Rustdoc, unconditionally, because this tool could not see it and the gate can.
#
# WHY: twice now a package has gone green here and red on the gate's
# `lint / rustdoc` job, both times for the same shape — a **public** doc
# comment linking a **private** item, which trips
# `-D rustdoc::private-intra-doc-links`. Migration 15 did it, and
# `memory/inject.rs` did it again two batches later, reaching `main` because
# this script reported every traced target passing.
#
# The recorded rule was "a schema change needs the full gate, blast-radius
# green is not sufficient". That was too narrow: the real gap is that this
# tool maps changed files to *cargo test targets*, and rustdoc is not one.
# Any package adding a doc comment can be red in a job nothing here runs.
#
# It costs about eight seconds and it closes the whole class.
echo
printf '\033[1m=== cargo doc --no-deps (rustdoc) ===\033[0m\n'
doc_out="$(mktemp)"
if RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p glasshouse >"$doc_out" 2>&1; then
  echo "  rustdoc: clean"
else
  rc=1
  echo "  --- rustdoc failures ---"
  grep -E '^(error|warning)' "$doc_out" | head -12
fi
rm -f "$doc_out"

echo
if [ "$rc" -eq 0 ]; then
  printf '\033[32mblast-radius: every traced target passed\033[0m\n'
else
  printf '\033[31mblast-radius: FAILURES above — fix before the gate\033[0m\n'
fi
exit "$rc"
