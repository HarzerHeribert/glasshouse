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
#   test / msrv     WINDOWS  -> NOT COVERED. See below.
#
# Five of the seven CI jobs. The two Windows jobs cannot be run on this
# machine, and nothing here should be read as evidence about Windows. This
# project has already shipped one Windows-only defect that only
# `test (windows-latest)` caught, and Phase 4's interrupt box can be closed by
# nothing else. `--windows` runs a cross-compile check, which proves the
# Windows code path still *compiles* and proves nothing whatever about whether
# it works.
#
# USAGE
#   scripts/ci-local.sh              # macOS + Linux  (the default gate)
#   scripts/ci-local.sh --macos      # native jobs only, fastest
#   scripts/ci-local.sh --linux      # container jobs only
#   scripts/ci-local.sh --windows    # add the compile-only cross check
#   scripts/ci-local.sh --flake      # measure the pty flake rate (FLAKE_RUNS=10)
#   scripts/ci-local.sh --windows-vm # real Windows, via GLASSHOUSE_WINDOWS_HOST
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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

RESULTS=()
FAILED=0

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
  step "lint / evidence coverage" python3 scripts/check-evidence-coverage.py --strict
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
        -e CARGO_TERM_COLOR=always \
        -e STEP="$1" \
        rust:latest bash -c '
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
          tar -C /src --exclude=./target -cf - . | tar -C /home/ci/repo -xf -
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

# --- Windows, for real, when a VM is available -------------------------------
#
# The one gap nothing local could close: Windows containers need a Windows
# kernel, and this host is linux/aarch64. A Windows VM is the only local route,
# and it is the only thing that can close Phase 4's interrupt box — every
# interrupt test in the suite is `#[cfg(unix)]`, so a green `test
# (windows-latest)` has always been the absence of evidence wearing the same
# colour.
#
# Set GLASSHOUSE_WINDOWS_HOST to an ssh destination for the VM and this becomes
# a real gate job. Unset, it says so and skips — it never pretends.
#
#   GLASSHOUSE_WINDOWS_HOST=eneas@win-vm scripts/ci-local.sh --windows-vm
#
# The VM needs: rustup with the msvc toolchain, git, and a checkout path given
# by GLASSHOUSE_WINDOWS_PATH (default C:/glasshouse). The tree is pushed with
# rsync-over-ssh rather than shared, for the same reason the Linux leg copies
# instead of mounting: a shared source tree is a wrong-green waiting to happen.
if [ "$DO_WINVM" -eq 1 ]; then
  if [ -z "${GLASSHOUSE_WINDOWS_HOST:-}" ]; then
    RESULTS+=("SKIP  windows VM — set GLASSHOUSE_WINDOWS_HOST=user@host")
  else
    WINPATH="${GLASSHOUSE_WINDOWS_PATH:-C:/glasshouse}"
    win_run() {
      # shellcheck disable=SC2029
      ssh "$GLASSHOUSE_WINDOWS_HOST" "cd $WINPATH && $1"
    }
    step "windows VM / sync" sh -c \
      "rsync -az --delete --exclude target --exclude .git ./ '$GLASSHOUSE_WINDOWS_HOST:$WINPATH/'"
    step "windows VM / build" win_run \
      'set RUSTFLAGS=-D warnings && cargo build --locked --workspace --all-targets'
    step "windows VM / test" win_run \
      'set RUSTFLAGS=-D warnings && cargo test --locked --workspace'
    # The reason the VM exists. These are `#[cfg(windows)]` paths that have
    # never executed anywhere: ConPTY's input mode, and interrupt delivery.
    step "windows VM / pty + interrupt" win_run \
      'cargo test --locked -p glasshouse --test pty_smoke -- --nocapture'
  fi
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
    step "windows CROSS-CHECK (compiles only, proves nothing about behaviour)" \
      cargo check --locked --workspace --target "$TARGET"
  else
    RESULTS+=("SKIP  windows cross-check — rustup target add $TARGET (and brew install mingw-w64)")
  fi
fi

printf '\n\033[1m=== summary ===\033[0m\n'
printf '%s\n' "${RESULTS[@]}"
if [ "$DO_WIN" -eq 0 ]; then
  printf '\n\033[33mNOTE\033[0m  Windows was not exercised at all. Nothing here is evidence about Windows.\n'
else
  printf '\n\033[33mNOTE\033[0m  The Windows check compiles the target; it does not run a single test there.\n'
fi
[ "$FAILED" -eq 0 ] || printf '\n\033[31mCI-LOCAL FAILED\033[0m\n'
exit "$FAILED"
