#!/usr/bin/env bash
# scripts/mutate.sh — practice §16's mutation-testing ritual, as one command
# that refuses to leave the tree dirty.
#
# WHY THIS EXISTS
# ---------------
# §16 requires six manual steps: back up the file, apply the mutation, touch
# it (cargo's mtime granularity has bitten this project — restoring with `mv`
# or `cp` without moving the timestamp gets a stale binary judged fresh, §16),
# rebuild, run the named test, confirm it FAILS, restore, and diff
# byte-identical. Batch 45's integrator ran four of these by hand; each was
# several tool calls, and the byte-identical restore was enforced only by
# discipline. This does all of it in order, and it ALWAYS restores from its
# backup — including on interrupt, via a trap — then refuses to finish
# quietly if the restored file is not byte-identical to what it started with.
#
# A KILLED mutation confirms a test proves what it claims. A SURVIVED
# mutation is the more valuable result: it names a behaviour no test in the
# given command actually watches. This tool says so loudly rather than
# treating SURVIVED as the boring outcome.
#
# USAGE
#   scripts/mutate.sh --file <path> --find <literal> --replace <literal> \
#                      (--test <cargo test args...> | --test-cmd <command...>) \
#                      [--name <label>] [--expect-survive] [--allow-dirty]
#
#   scripts/mutate.sh --script <file> [--expect-survive] [--allow-dirty]
#     Runs several mutations as one batch, so a whole package's mutations are
#     one invocation. Each non-blank, non-'#' line of <file> is five
#     TAB-separated fields:
#         file<TAB>find<TAB>replace<TAB>name<TAB>cargo-test-args
#     find/replace must not themselves contain a literal tab. Every line
#     still gets the full ritual — its own backup, its own restore, its own
#     byte-identical check — nothing is shared between lines but the flags.
#
# --test-cmd exists so THIS script's own tests don't need a build: it runs an
# arbitrary command in place of `cargo test`, and only that command's exit
# code decides KILLED vs SURVIVED. Documented here because it is a deliberate
# escape hatch, not an accident: `scripts/tests/test_mutate.py` uses it with
# a trivially-passing and a trivially-failing command so the acceptance tests
# run in under a second. It resolves them portably rather than naming
# `/bin/true` and `/bin/false`, which this comment used to do and which is
# wrong on macOS -- they live in `/usr/bin/` there, and a missing binary exits
# 127, which this script correctly reports as KILLED. That cost the
# orchestrator a confused investigation on 2026-08-30: the verdict was right
# and the comment that suggested the probe was not.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Resolve which tree this invocation operates on. $REPO above is the SCRIPT's
# own location, not necessarily the CALLER's tree: scripts/ is tracked, so
# every worktree has its own copy, and this script is reachable by absolute
# path / PATH from the main checkout. Every mutation below is driven off
# $REPO (relative --file resolution, the dirty-file git -C query, and the cd
# inside run_test), so leaving it pointed at the wrong tree mutates one tree
# and tests another. This is the same defect scripts/blast-radius.sh fixed
# first; this block follows its shape.
#
# Kinship is git-common-dir, not a path prefix: a worktree of this repo
# shares one git dir with the main checkout no matter where it lives.
common_dir() {                     # absolute common .git dir for tree "$1"
  local d
  d="$(git -C "$1" rev-parse --git-common-dir 2>/dev/null)" || return 1
  case "$d" in
    # git reports the common dir through a resolved (physical) path when it
    # is already absolute; `pwd -P` matches that for the relative case so the
    # two forms compare equal instead of differing by a symlinked tmp/mount
    # prefix (e.g. macOS /var vs /private/var).
    /*) printf '%s\n' "$d" ;;
    *)  (cd "$1/$d" 2>/dev/null && pwd -P) ;;
  esac
}

ORIG_CWD="$(pwd)"
CALLER_TOPLEVEL="$(git -C "$ORIG_CWD" rev-parse --show-toplevel 2>/dev/null)"

if [ -z "$CALLER_TOPLEVEL" ]; then
  echo "mutate.sh: refusing -- '$ORIG_CWD' is not a git worktree (script lives at '$REPO')" >&2
  exit 1
fi

# Compare through git's own (symlink-resolved) view of $REPO, not the logical
# BASH_SOURCE-derived path, so a caller reached through a symlinked mount does
# not spuriously look like "a different tree".
REPO_TOPLEVEL="$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null)"

if [ "$CALLER_TOPLEVEL" != "$REPO_TOPLEVEL" ]; then
  REPO_COMMON="$(common_dir "$REPO")"
  CALLER_COMMON="$(common_dir "$CALLER_TOPLEVEL")"
  if [ -z "$REPO_COMMON" ] || [ "$REPO_COMMON" != "$CALLER_COMMON" ]; then
    echo "mutate.sh: refusing -- '$CALLER_TOPLEVEL' is not a worktree of the repo at '$REPO'" >&2
    exit 1
  fi
  echo "mutate.sh: operating on the caller's worktree at $CALLER_TOPLEVEL (not $REPO)"
  REPO="$CALLER_TOPLEVEL"
fi

usage() {
  cat >&2 <<'EOF'
usage: mutate.sh --file <path> --find <literal> --replace <literal> \
                  (--test <cargo-args...> | --test-cmd <cmd...>) \
                  [--name <label>] [--expect-survive] [--allow-dirty]
       mutate.sh --script <file> [--expect-survive] [--allow-dirty]

--test and --test-cmd consume the rest of the command line, so put
--name/--expect-survive/--allow-dirty before them.
EOF
  exit 2
}

FILE=""
FIND=""
REPLACE=""
NAME=""
SCRIPT=""
EXPECT_SURVIVE=0
ALLOW_DIRTY=0
MODE=""
TEST_ARGS=()
TEST_CMD=()

while [ $# -gt 0 ]; do
  case "$1" in
    --file) FILE="${2:-}"; shift 2 ;;
    --find) FIND="${2:-}"; shift 2 ;;
    --replace) REPLACE="${2:-}"; shift 2 ;;
    --name) NAME="${2:-}"; shift 2 ;;
    --script) SCRIPT="${2:-}"; shift 2 ;;
    --expect-survive) EXPECT_SURVIVE=1; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    --test) shift; MODE="cargo"; TEST_ARGS=("$@"); break ;;
    --test-cmd) shift; MODE="cmd"; TEST_CMD=("$@"); break ;;
    -h|--help) usage ;;
    *) echo "mutate.sh: unknown argument: $1" >&2; usage ;;
  esac
done

# Run the configured test. Returns the test's own exit status: 0 == passed
# (mutation SURVIVED), non-zero == failed (mutation KILLED).
run_test() {
  local out="$1"
  if [ "$MODE" = "cmd" ]; then
    ( cd "$REPO" && "${TEST_CMD[@]}" ) >"$out" 2>&1
  else
    ( cd "$REPO" && cargo test "${TEST_ARGS[@]}" ) >"$out" 2>&1
  fi
}

# Perform the whole ritual for one mutation. Returns 0 on KILLED (or SURVIVED
# with --expect-survive), 1 on an unexpected SURVIVED, 2 on a setup refusal
# (bad occurrence count, dirty tree, missing file).
mutate_one() {
  local file="$1" find="$2" replace="$3" name="$4"

  if [ -z "$file" ] || [ -z "$find" ]; then
    echo "mutate.sh: --file and --find are required" >&2
    return 2
  fi
  if [ -z "$MODE" ]; then
    echo "mutate.sh: need --test <cargo-args...> or --test-cmd <command...>" >&2
    return 2
  fi

  local path
  if [[ "$file" = /* ]]; then
    path="$file"
  else
    path="$REPO/$file"
  fi
  if [ ! -f "$path" ]; then
    echo "mutate.sh: no such file: $file" >&2
    return 2
  fi

  # Step 1: refuse an already-dirty file — mutating it makes restore ambiguous.
  if [ "$ALLOW_DIRTY" -ne 1 ]; then
    local dirty
    dirty="$(git -C "$REPO" status --porcelain -- "$path" 2>/dev/null)"
    if [ -n "$dirty" ]; then
      echo "mutate.sh: $path already has uncommitted changes; pass --allow-dirty to proceed anyway" >&2
      return 2
    fi
  fi

  # Step 2: back up outside the repo.
  local backup_dir backup
  backup_dir="$(mktemp -d)"
  backup="$backup_dir/mutate-backup"
  cp "$path" "$backup"

  local restored=0
  restore() {
    [ "$restored" -eq 1 ] && return
    restored=1
    cp "$backup" "$path"
    # §16: a restore that does not move the mtime gets judged fresh against a
    # mutant binary by the next build. Touch unconditionally on the way back.
    touch "$path"
    if ! cmp -s "$backup" "$path"; then
      echo "mutate.sh: FATAL — restore of $path did not come back byte-identical to its backup ($backup)" >&2
      exit 70
    fi
  }
  trap restore EXIT

  # Step 3: apply, and only if the find string occurs exactly once. Silent
  # partial application is the failure mode this must not have, so this is a
  # Python string count/replace rather than a line-oriented sed/grep — it
  # counts every occurrence correctly even for a multi-line find string.
  local apply_rc=0
  python3 - "$path" "$find" "$replace" <<'PY' || apply_rc=$?
import sys
path, find, replace = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path, encoding="utf-8").read()
count = text.count(find)
if count != 1:
    print(f"mutate.sh: refusing — find string occurs {count} time(s) in {path}, need exactly 1", file=sys.stderr)
    sys.exit(3)
open(path, "w", encoding="utf-8").write(text.replace(find, replace, 1))
PY
  if [ "$apply_rc" -ne 0 ]; then
    restore
    trap - EXIT
    rm -rf "$backup_dir"
    return 2
  fi

  # Step 4: force a rebuild.
  touch "$path"

  # Step 5: run the test.
  local out
  out="$(mktemp)"
  local label="${name:-$(basename "$path")}"
  local verdict
  if run_test "$out"; then
    verdict=SURVIVED
  else
    verdict=KILLED
  fi

  # Step 6: decide, loudly.
  if [ "$verdict" = KILLED ]; then
    echo "mutation $label: KILLED"
    local failing
    failing="$(grep -E '^test .*\.\.\. FAILED$' "$out")"
    [ -n "$failing" ] && printf '%s\n' "$failing"
    local assertion
    assertion="$(grep -E 'panicked at|assertion' "$out" | head -3)"
    [ -n "$assertion" ] && printf '%s\n' "$assertion"
  else
    echo "mutation $label: SURVIVED"
    echo "  Every test in this command still passed with the mutation in place."
    echo "  THIS IS THE MOST VALUABLE OUTCOME: it names behaviour the suite is"
    echo "  not actually watching — e.g. a check that inspects a return code"
    echo "  but never the value it claims to validate."
    # §68, and the integrator walked straight into it on this tool's second
    # real use: a SURVIVED verdict from a command that never ran the relevant
    # test is indistinguishable from a genuine survival. The mutation was in
    # main.rs and the command said `--test checkpoint_portability`, a target
    # that does not contain main.rs's own tests; it reported SURVIVED, and
    # re-running with `--bin glasshouse` reported KILLED.
    #
    # So always show what actually ran. A count that looks too small, or a
    # "0 filtered out" against a whole-suite expectation, is the reader's cue
    # that the command — not the code — is what survived.
    local counts
    counts="$(grep -E '^test result:' "$out")"
    if [ -n "$counts" ]; then
      echo
      echo "  WHAT ACTUALLY RAN — check this before believing the verdict:"
      printf '    %s\n' "$counts"
      echo "  If the relevant test is not in there, the COMMAND survived, not"
      echo "  the code. Re-run naming the target that holds it."
    else
      echo
      echo "  WARNING: no 'test result:' line was produced at all, so it is not"
      echo "  even established that any test ran. Treat this verdict as void."
    fi
  fi

  # Step 7: always restore, and fail loudly if the restore drifted.
  restore
  trap - EXIT
  rm -f "$out"
  rm -rf "$backup_dir"

  # Step 8: the line a worker pastes into its glasshouse-facts block.
  echo "glasshouse-facts: mutate $label -> $verdict (file=$file)"

  if [ "$verdict" = SURVIVED ] && [ "$EXPECT_SURVIVE" -ne 1 ]; then
    return 1
  fi
  return 0
}

run_script() {
  local script_file="$1"
  if [ ! -f "$script_file" ]; then
    echo "mutate.sh: no such script: $script_file" >&2
    exit 2
  fi
  local any_fail=0
  local sfile sfind sreplace sname stestargs
  while IFS=$'\t' read -r sfile sfind sreplace sname stestargs || [ -n "${sfile:-}" ]; do
    [ -z "${sfile:-}" ] && continue
    case "$sfile" in \#*) continue ;; esac
    MODE="cargo"
    # shellcheck disable=SC2206  # intentional word-splitting of test args
    TEST_ARGS=($stestargs)
    if ! mutate_one "$sfile" "$sfind" "$sreplace" "$sname"; then
      any_fail=1
    fi
  done < "$script_file"
  return "$any_fail"
}

if [ -n "$SCRIPT" ]; then
  run_script "$SCRIPT"
  exit $?
fi

[ -n "$FILE" ] || usage
[ -n "$FIND" ] || usage

mutate_one "$FILE" "$FIND" "$REPLACE" "$NAME"
exit $?
