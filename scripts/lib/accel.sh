# shellcheck shell=bash
#
# Compiler-cache setup, shared by scripts/ci-local.sh and
# scripts/blast-radius.sh. Source it; it exports and returns.
#
# WHY THIS EXISTS
# ---------------
# The parallel-worker workflow creates a fresh worktree per worker
# (scripts/dev/new-worker.sh) and reaps it afterwards (scripts/reap-worktrees.sh).
# A worktree's target/ is not shared with any other tree and dies with it, so
# on 2026-09-04 every one of the ~20 sibling worktrees under ~/projects had NO
# target/ at all while the main checkout held 60G. Every worker was paying a
# full cold compile of the dependency graph and the library before it could run
# one test, and paying it again next time because nothing outlived the tree.
#
# sccache fixes exactly that shape: the cache is keyed on (compiler, flags,
# preprocessed input), lives in the user's cache directory rather than in any
# target/, and is therefore shared by every worktree, every worker, and every
# gate run on this machine. The main checkout's warm target/ is unaffected --
# cargo still short-circuits on freshness before sccache is ever consulted.
#
# ABSENCE IS NOT AN ERROR
# -----------------------
# `build.rustc-wrapper` in .cargo/config.toml would have been shorter and is
# the wrong mechanism: it is unconditional, so a machine without sccache --
# a fresh clone, CI, the rust:$TOOLCHAIN container -- fails to build at all,
# with an error that names a tool nobody asked for. This file no-ops instead,
# and says so once, so the gate's behaviour is identical on a machine that has
# never heard of sccache.
#
# THE INCREMENTAL TRADE
# ---------------------
# sccache cannot cache an incremental compilation and forwards it straight to
# rustc, so leaving `CARGO_INCREMENTAL` at its dev-profile default would mean a
# 0% hit rate on exactly the builds this exists to speed up. It is set to 0
# here, which is what a gate wants anyway -- a gate compiles a tree once and
# throws the state away, and that is the case incremental is worst at.
#
# This is scoped to the gate scripts on purpose. An interactive `cargo build`
# in the main checkout does not source this file and keeps incremental,
# because a warm tree editing one function is the case incremental is best at.
# Both defaults are right for their own workload; the mistake would be picking
# one globally.

accel_enable() {
  if ! command -v sccache >/dev/null 2>&1; then
    if [ -z "${ACCEL_QUIET:-}" ]; then
      echo "accel: sccache not installed -- building without a compiler cache"
    fi
    return 0
  fi

  # Already wrapped by a caller (nested gate invocation, or a worker that set
  # it deliberately). Do not stack wrappers: sccache invoking sccache is a
  # confusing failure, not a faster build.
  if [ -n "${RUSTC_WRAPPER:-}" ]; then
    return 0
  fi

  export RUSTC_WRAPPER="sccache"
  export CARGO_INCREMENTAL=0
  # 10G is the sccache default and is not enough here: 160 integration-test
  # targets across ~20 worktrees and two compilers (the Homebrew default and
  # the pinned [workspace.metadata.ci] toolchain, which key separately) evict
  # each other out of a default cache and turn a hit into a silent miss.
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-40G}"
  # The server exits after ten idle minutes by default and takes its counters
  # with it. A full gate spends longer than that in the Linux container and
  # the Windows VM, neither of which touches the host's sccache, so the report
  # at the end used to read 0 hits / 0 misses after a run that had in fact
  # populated the cache -- measured 2026-09-05, first run after enabling it.
  # Cache DATA is on disk and survives; only the tally lives in the server.
  # 0 means never exit while idle; the process is small and has no work to do.
  export SCCACHE_IDLE_TIMEOUT="${SCCACHE_IDLE_TIMEOUT:-0}"
  # A fixed, named location rather than sccache's default under
  # ~/Library/Caches. macOS "free up space", cleaner tools and `brew cleanup`
  # style sweeps treat that directory as disposable, and losing this cache is
  # not a crash -- it is every worktree going cold again with no message
  # saying why. Naming the directory after the project also keeps it out of
  # any other sccache user's eviction budget on this machine.
  export SCCACHE_DIR="${SCCACHE_DIR:-$HOME/.glasshouse/sccache}"

  if [ -z "${ACCEL_QUIET:-}" ]; then
    echo "accel: sccache $(sccache --version 2>/dev/null | awk '{print $2}') enabled (CARGO_INCREMENTAL=0, cache ${SCCACHE_CACHE_SIZE})"
  fi
}

# Print the hit rate for the work done since accel_enable, so a gate run says
# whether the cache actually helped rather than only that it was switched on.
# A gate that cannot report its own effect is how a "speedup" survives being
# useless -- see scripts/ci-local.sh's own §20 note about gates that cannot fail.
accel_report() {
  [ -n "${RUSTC_WRAPPER:-}" ] || return 0
  command -v sccache >/dev/null 2>&1 || return 0
  local hits misses rate
  hits="$(sccache --show-stats 2>/dev/null | awk '/^Cache hits /{print $3; exit}')"
  misses="$(sccache --show-stats 2>/dev/null | awk '/^Cache misses /{print $3; exit}')"
  rate="$(sccache --show-stats 2>/dev/null | awk '/^Cache hits rate/{print $4; exit}')"
  [ -n "$hits" ] || return 0
  # A run whose every compile happened somewhere else -- the Linux container,
  # the Windows VM -- has nothing to report, and "0 hits / 0 misses" reads as
  # a broken cache rather than an unused one. It did exactly that once.
  [ "$((hits + misses))" -gt 0 ] || return 0
  echo "accel: sccache ${hits} hits / ${misses} misses (${rate:-n/a}) since this session started"
}
