#!/usr/bin/env bash
# Run the CI matrix locally, because GitHub Actions minutes are exhausted.
#
# WHY THIS EXISTS
# ---------------
# From 2026-08-26 the Actions quota for this private repository is spent until
# it resets. A push still creates a run; every job fails instantly with no
# steps and no log, which looks exactly like a broken build and is not one.
# Until the quota returns, THIS script is the gate. Run it before every commit.
#
# It mirrors .github/workflows/ci.yml deliberately and closely — `--locked`,
# `RUSTFLAGS=-D warnings` on build and test, clippy without `--all-features`,
# and the README progress check — because a local gate that tests something
# easier than CI is not a gate, it is a rehearsal. If you change ci.yml, change
# this in the same commit.
#
# WHAT IT COVERS, HONESTLY
#   lint            ubuntu   -> Linux container
#   test / msrv     ubuntu   -> Linux container
#   test / msrv     macOS    -> this machine, natively
#   test / msrv     WINDOWS  -> only with --windows-vm, and only if the VM is up.
#
# The default run is five of the seven CI jobs, and nothing in it is evidence
# about Windows. `--windows` adds a cross-compile check, which proves the
# Windows code path still *compiles* and proves nothing whatever about whether
# it works. `--windows-vm` is the only mode that runs Windows for real; it is
# opt-in because the VM has to be booted by hand and costs this machine's CPU
# and memory. The summary's closing NOTE says which of those three happened.
#
# HOW LONG IT TAKES, AND WHY (measured 2026-08-29, 12-core M-series)
#   Warm, the whole default gate is ~2–4 minutes and nearly all of it is test
#   execution: ~80s per platform, of which terminal_loss (24s) and
#   session_supervision (14s) are timer-bound — identical to 0.1s at load 2.6
#   and load 5.5. Every compile step is 0–2s warm. A fresh worktree with no
#   target/ and no Linux volume is a full rebuild on both sides and still
#   finished in 135s. Seeding a worktree's caches from main (APFS clone of
#   target/, copy of the Linux volume) was built, proven sound with a planted
#   failure that FAILed the same test cold and seeded, and then rejected: it
#   cost 88s to seed and saved 13s. Do not re-try it without new numbers.
#
# USAGE
#   scripts/ci-local.sh              # macOS + Linux  (the default gate)
#   scripts/ci-local.sh --macos      # native jobs only, fastest
#   scripts/ci-local.sh --linux      # container jobs only
#   scripts/ci-local.sh --windows    # add the compile-only cross check
#   scripts/ci-local.sh --flake      # measure the pty flake rate (FLAKE_RUNS=10)
#   scripts/ci-local.sh --windows-vm # real Windows on the ARM64 VM
set -uo pipefail

ORIG_CWD="$(pwd)"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# $REPO above is the SCRIPT's own location, not necessarily the CALLER's
# tree: scripts/ is tracked, so every worktree has its own copy, and this is
# THE gate to run before every commit -- it must build and test the caller's
# OWN tree, not whichever copy happens to be invoked. Reproduced 2026-08-30
# (script-tree-audit): run via absolute path from a worktree -- the exact
# form CLAUDE.md itself recommends for other scripts in this project, to
# route around a DIFFERENT worktree-resolution bug -- this cd'd into and
# would have tested the main checkout instead, silently. Same shape and same
# fix as scripts/blast-radius.sh.
common_dir() {
  local d
  d="$(git -C "$1" rev-parse --git-common-dir 2>/dev/null)" || return 1
  case "$d" in
    /*) printf '%s\n' "$d" ;;
    *)  (cd "$1/$d" 2>/dev/null && pwd -P) ;;
  esac
}

CALLER_TOPLEVEL="$(git -C "$ORIG_CWD" rev-parse --show-toplevel 2>/dev/null)"
if [ -n "$CALLER_TOPLEVEL" ]; then
  REPO_TOPLEVEL="$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null)"
  if [ "$CALLER_TOPLEVEL" != "$REPO_TOPLEVEL" ]; then
    REPO_COMMON="$(common_dir "$REPO")"
    CALLER_COMMON="$(common_dir "$CALLER_TOPLEVEL")"
    if [ -n "$REPO_COMMON" ] && [ "$REPO_COMMON" = "$CALLER_COMMON" ]; then
      echo "ci-local: testing the caller's worktree at $CALLER_TOPLEVEL (not $REPO)"
      REPO="$CALLER_TOPLEVEL"
    fi
  fi
fi

cd "$REPO" || exit 1

DO_MAC=0; DO_LINUX=0; DO_WIN=0; DO_FLAKE=0; DO_WINVM=0
if [ $# -eq 0 ]; then DO_MAC=1; DO_LINUX=1; fi
for a in "$@"; do
  case "$a" in
    --macos)   DO_MAC=1 ;;
    --linux)   DO_LINUX=1 ;;
    --windows) DO_WIN=1 ;;
    --flake)   DO_FLAKE=1 ;;
    --windows-vm) DO_WINVM=1 ;;
    --all)     DO_MAC=1; DO_LINUX=1; DO_WIN=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

MSRV="$(grep -m1 '^rust-version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
[ -n "$MSRV" ] || { echo "could not read rust-version from Cargo.toml" >&2; exit 2; }
# The stable compiler, declared once in [workspace.metadata.ci] and read by
# ci.yml the same way. It pins the Linux image below: `rust:latest` floated,
# and every bump silently threw away the whole Linux build cache.
TOOLCHAIN="$(grep -m1 '^toolchain' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
[ -n "$TOOLCHAIN" ] || { echo "could not read [workspace.metadata.ci] toolchain from Cargo.toml" >&2; exit 2; }

RESULTS=()
FAILED=0
# Whether Windows was actually exercised, and how. Both feed the closing NOTE,
# which must never claim more or less than the run earned.
WIN_VM_RAN=0
WIN_CROSS_RAN=0

step() {           # step <label> <command...>
  local label="$1"; shift
  printf '\n\033[1m=== %s\033[0m\n' "$label"
  if "$@"; then
    RESULTS+=("PASS  $label")
  else
    RESULTS+=("FAIL  $label")
    FAILED=1
  fi
}

# --- native macOS jobs -------------------------------------------------------
if [ "$DO_MAC" -eq 1 ]; then
  step "lint / fmt" cargo fmt --all -- --check
  step "lint / clippy" env RUSTFLAGS= cargo clippy --locked --workspace --all-targets -- -D warnings
  step "lint / rustdoc" env RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
  step "lint / README progress" python3 scripts/progress.py --check
  # Free-because-local checks. These never ran on GitHub Actions; they exist
  # because a local gate can afford questions a metered one could not.
  step "lint / doc boundary" scripts/check-doc-boundary.sh
  step "lint / evidence coverage" python3 scripts/check-evidence-coverage.py --strict --strict-consistency
  # The orchestration scripts have tests and, until 2026-08-27, nothing ran
  # them. validate_round.py gates every round and worker-watch.sh decides
  # when a worker is finished; both are cheap to break and expensive to
  # have wrong.
  step "lint / script tests" sh -c 'for t in scripts/tests/test_*.py; do python3 "$t" || exit 1; done'
  step "test (macos) / build" env RUSTFLAGS='-D warnings' cargo build --locked --workspace --all-targets
  step "test (macos) / test"  env RUSTFLAGS='-D warnings' sh -c 'cargo test --locked --workspace -- --nocapture < /dev/null'
  # Call the project's own script rather than `cargo +$MSRV`: its header
  # documents three traps, and `cargo +<v>` needs the rustup shim, which is
  # exactly how the first version of this file got a false red.
  step "msrv (macos) $MSRV" scripts/msrv-check.sh
fi

# --- Linux jobs, in a container ---------------------------------------------
if [ "$DO_LINUX" -eq 1 ]; then
  if ! docker info >/dev/null 2>&1; then
    RESULTS+=("SKIP  linux jobs — Docker is not running")
  else
    # A separate CARGO_TARGET_DIR inside the container: Linux artifacts must
    # never land in the host's target/, and a shared one would make every
    # local build recompile the world in both directions.
    # Volumes are keyed to THIS worktree. One shared pair was wrong twice
    # over: two team leads running this concurrently raced on the same
    # /home/ci, and a lead's worktree left files behind that then compiled
    # into main's build. A build cache may be shared; a source tree may not.
    TAG="$(printf '%s' "$REPO" | shasum | cut -c1-12)"
    docker volume create "glasshouse-ci-home-$TAG" >/dev/null 2>&1
    docker volume create glasshouse-ci-registry >/dev/null 2>&1
    # rustup's home, so clippy/rustfmt and the MSRV toolchain install once
    # rather than on every run (measured ~10s and a download each). Keyed by
    # the pinned toolchain: an empty named volume is seeded from the image on
    # first mount, and a volume seeded from rust:1.98 would otherwise keep
    # serving 1.98 as "default" under a later image. Toolchains are not
    # source-dependent, so unlike /home/ci this one is shared by every worktree.
    docker volume create "glasshouse-ci-rustup-$TOOLCHAIN" >/dev/null 2>&1
    # Two things here are not incidental; both were found by this script
    # producing a red on a tree that real ubuntu-latest had passed.
    #
    #  1. The tree is COPIED in, not bind-mounted. A test that writes an
    #     executable and immediately spawns it gets ETXTBSY ("Text file busy")
    #     across a macOS->Linux bind mount, which looks like a product defect
    #     and is not one.
    #  2. The build runs as a NON-ROOT user. `chmod 000` does not stop root,
    #     so a test asserting a directory cannot be listed passes vacuously —
    #     and one of this project's tests says so in its own failure message.
    #
    # target/ is excluded from the copy; it holds macOS artifacts and is large.
    run_linux() {
      docker run --rm \
        -v "$REPO":/src:ro \
        -v "glasshouse-ci-home-$TAG":/home/ci \
        -v glasshouse-ci-registry:/usr/local/cargo/registry \
        -v "glasshouse-ci-rustup-$TOOLCHAIN":/usr/local/rustup \
        -e CARGO_TERM_COLOR=always \
        -e STEP="$1" \
        "rust:$TOOLCHAIN" bash -c '
          set -e
          id -u ci >/dev/null 2>&1 || useradd -m -u 1000 ci
          # Wipe before extracting. `tar -x` writes over a tree, it never
          # removes what is no longer in the source — so a file deleted (or
          # belonging to a different worktree) survives and compiles. That is
          # how tests/checkpoint_portability.rs from another branch broke a
          # build of main, and it could as easily have hidden a failure.
          # /home/ci/target is deliberately NOT wiped: it is the build cache.
          rm -rf /home/ci/repo
          mkdir -p /home/ci/repo
          tar -C /src --exclude=./target --exclude=./.worktrees -cf - . | tar -C /home/ci/repo -xf -
          chown -R ci:ci /home/ci
          # rustup/cargo homes are root-owned in the image; the msrv step
          # installs a toolchain and must be able to write them.
          chown -R ci:ci /usr/local/rustup /usr/local/cargo
          # The step arrives as $STEP in the environment and is never
          # interpolated into a quoted string. It used to be nested inside
          # su -c "…$1…", and a step containing RUSTFLAGS="-D warnings" closed
          # that string early: the command was mangled and its exit status
          # meaningless, so `test (ubuntu)` reported PASS on a tree that had
          # just failed by hand. A gate that cannot fail is not a gate (§20).
          runuser -u ci -- bash -c '"'"'cd /home/ci/repo && export CARGO_TARGET_DIR=/home/ci/target && eval "$STEP"'"'"'
        '
    }
    step "test (ubuntu) / build+test" run_linux \
      'set -e; rustup component add clippy rustfmt >/dev/null 2>&1 || true;
       RUSTFLAGS="-D warnings" cargo build --locked --workspace --all-targets;
       RUSTFLAGS="-D warnings" cargo test --locked --workspace -- --nocapture < /dev/null'
    step "lint (ubuntu) / clippy" run_linux \
      'set -e; rustup component add clippy >/dev/null 2>&1 || true;
       cargo clippy --locked --workspace --all-targets -- -D warnings'
    step "msrv (ubuntu) $MSRV" run_linux \
      "rustup toolchain install $MSRV --profile minimal && scripts/msrv-check.sh"
  fi
fi

# --- Windows, for real, on the ARM64 VM --------------------------------------
#
# The one gap nothing local could close: Windows containers need a Windows
# kernel, and this host is linux/aarch64. A Windows 11 ARM64 VM is the only
# local route, and it is the only thing that can close Phase 4's interrupt box
# — every interrupt test in the suite is `#[cfg(unix)]`, so a green
# `test (windows-latest)` has always been the absence of evidence wearing the
# same colour.
#
# This drives `glasshouse-windows-ci`, the host helper that owns everything
# about reaching the VM: it finds the guest's DHCP address, packages the
# working tree (tracked *and* untracked, so uncommitted work is what gets
# tested), extracts it to C:\ci\glasshouse, keeps cargo artifacts in
# C:\ci\target, and returns a CI exit code. This script deliberately knows
# none of that. An earlier version here did its own rsync-and-ssh against a
# GLASSHOUSE_WINDOWS_HOST that never existed; two ways to reach one VM is one
# too many, and the one that had never run was the one to delete.
#
# The helper's exit codes are the whole contract:
#   0  the Windows job passed
#   2  the helper refused before running anything — no VM, no key, no lease
#   *  the Windows job ran and failed
# So a VM that is not booted SKIPs with the helper's own reason. It never
# fails the gate for being absent, and it never passes for being absent
# either.
if [ "$DO_WINVM" -eq 1 ]; then
  WIN_HELPER="$(command -v glasshouse-windows-ci 2>/dev/null)"
  WIN_UNAVAILABLE=""
  if [ -z "$WIN_HELPER" ]; then
    WIN_UNAVAILABLE="glasshouse-windows-ci is not on PATH"
  elif grep -q 'GLASSHOUSE_CI_REPO' "$WIN_HELPER"; then
    # The helper lets its caller choose the tree, so point it at this one.
    export GLASSHOUSE_CI_REPO="$REPO"
  else
    # It does not, so it packages one hardcoded checkout. That is correct in
    # that checkout and a wrong-green anywhere else: the run would report on
    # a tree nobody asked about, which is the same trap the Linux leg copies
    # instead of bind-mounting to avoid. Refuse rather than guess.
    WIN_HELPER_REPO="$(sed -n 's/^readonly ci_repo="\(.*\)"$/\1/p' "$WIN_HELPER" | head -1)"
    if [ "$WIN_HELPER_REPO" != "$REPO" ]; then
      WIN_UNAVAILABLE="glasshouse-windows-ci packages ${WIN_HELPER_REPO:-a checkout it does not name}, not this worktree; make its ci_repo honour \$GLASSHOUSE_CI_REPO"
    fi
  fi

  # One Windows job per `step` line, matching ci.yml's own granularity, so a
  # Windows failure reads the same way in the summary as any other job.
  win_step() {          # win_step <label> <helper-mode>
    local label="$1" mode="$2" status err
    if [ -n "$WIN_UNAVAILABLE" ]; then
      RESULTS+=("SKIP  $label — $WIN_UNAVAILABLE")
      return
    fi
    printf '\n\033[1m=== %s\033[0m\n' "$label"
    err="$(mktemp)"
    glasshouse-windows-ci "$mode" 2>"$err"; status=$?
    cat "$err" >&2
    if [ "$status" -eq 0 ]; then
      WIN_VM_RAN=1
      RESULTS+=("PASS  $label")
    elif [ "$status" -eq 2 ]; then
      # Refused before running anything. Carry the helper's own last words
      # into the summary and skip the remaining Windows jobs with it, rather
      # than asking a VM that is not there three more times.
      WIN_UNAVAILABLE="$(tail -n 1 "$err")"
      [ -n "$WIN_UNAVAILABLE" ] || WIN_UNAVAILABLE="the VM is not reachable; start it in VMware Fusion"
      RESULTS+=("SKIP  $label — $WIN_UNAVAILABLE")
    else
      WIN_VM_RAN=1
      RESULTS+=("FAIL  $label")
      FAILED=1
    fi
    rm -f "$err"
  }

  win_step "test (windows) / build" build
  win_step "test (windows) / test" test
  win_step "msrv (windows) $MSRV" msrv
fi

# --- flake rate: the standing debt needs a number, not a pass ----------------
#
# `pty_smoke::a_direct_provider_profile_reaches_a_real_child_and_only_that_child`
# still fails about once in 37 full-suite runs with the child killed by SIGABRT.
# One green pass says nothing about it. A local gate can afford to ask how often,
# which a metered one never could — so this runs the pty-sensitive suites N times
# and reports failures/attempts rather than a verdict.
if [ "$DO_FLAKE" -eq 1 ]; then
  RUNS="${FLAKE_RUNS:-10}"
  printf '\n\033[1m=== flake rate over %s runs ===\033[0m\n' "$RUNS"
  fails=0
  for i in $(seq 1 "$RUNS"); do
    if RUSTFLAGS='-D warnings' cargo test --locked -p glasshouse \
         --test pty_smoke --test events_lifecycle -- --nocapture < /dev/null >/dev/null 2>&1; then
      printf '  run %2s/%s ok\n' "$i" "$RUNS"
    else
      fails=$((fails + 1))
      printf '  run %2s/%s \033[31mFAILED\033[0m\n' "$i" "$RUNS"
    fi
  done
  RESULTS+=("RATE  pty flake: $fails failure(s) in $RUNS run(s)")
  # A rate is a measurement, not a verdict: it never fails the gate on its own.
fi

# --- Windows: compile-only, and labelled as such -----------------------------
if [ "$DO_WIN" -eq 1 ]; then
  TARGET=x86_64-pc-windows-gnu
  if rustup target list --installed | grep -q "$TARGET"; then
    WIN_CROSS_RAN=1
    step "windows CROSS-CHECK (compiles only, proves nothing about behaviour)" \
      cargo check --locked --workspace --target "$TARGET"
  else
    RESULTS+=("SKIP  windows cross-check — rustup target add $TARGET (and brew install mingw-w64)")
  fi
fi

printf '\n\033[1m=== summary ===\033[0m\n'
printf '%s\n' "${RESULTS[@]}"
# Three different true statements, and the run picks the one it earned. The
# old version keyed on `--windows` alone, so `--windows-vm` could run the
# whole Windows suite on a real machine and still be told Windows was not
# exercised at all — and a `--windows` whose target was not installed was
# told the opposite.
if [ "$WIN_VM_RAN" -eq 1 ]; then
  printf '\n\033[33mNOTE\033[0m  Windows ran for real on the ARM64 VM. Those lines ARE evidence about Windows.\n'
elif [ "$WIN_CROSS_RAN" -eq 1 ]; then
  printf '\n\033[33mNOTE\033[0m  The Windows check compiles the target; it does not run a single test there.\n'
else
  printf '\n\033[33mNOTE\033[0m  Windows was not exercised at all. Nothing here is evidence about Windows.\n'
fi
[ "$FAILED" -eq 0 ] || printf '\n\033[31mCI-LOCAL FAILED\033[0m\n'
exit "$FAILED"
