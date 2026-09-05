# shellcheck shell=bash
#
# A private, unlocked Secret Service provider for the Linux test lane, so
# `secret_native`'s round trip (Phase 9E line 442) runs for real on Linux
# instead of skipping. Source it, then call `secret_service_fixture_run`
# with the command to run under it.
#
# WHY THIS EXISTS
# ---------------
# The Linux `backend::probe` in `crates/glasshouse/src/secret/native.rs`
# refuses rather than waits, and `tests/secret_native.rs`'s round trips
# skip loudly whenever `detect()` refuses -- which it always did before this
# file, because neither the Linux container nor a hosted runner has ever had
# a Secret Service to reach. Skipping is the correct behaviour when no
# provider exists; it is not evidence the round trip works. This starts one.
#
# TWO PLACES, ONE SHAPE
# ----------------------
# `scripts/ci-local.sh`'s `run_linux()` and `.github/workflows/ci-extended.yml`'s
# Linux `test` step both source this file and call the same function, so the
# local gate and the trailing sweep exercise the identical fixture. Neither
# touches macOS or Windows.
#
# NOTHING OUTLIVES THE CALL
# --------------------------
# `dbus-run-session` opens a private session bus for the duration of the
# wrapped command and tears it down -- daemon included -- the instant that
# command exits, so the keyring never survives past one `cargo test`
# invocation. Both `XDG_DATA_HOME` (the keyring's own data, e.g.
# `keyrings/login.keyring`) and `XDG_CACHE_HOME` (`gnome-keyring-daemon`'s
# control-socket directory, `keyring-XXXXXX/control`) are pointed at the same
# fresh tmpdir for the call, so nothing is ever written under the real
# `$HOME/.cache` or `$HOME/.local/share/keyrings` -- which in `ci-local.sh`'s
# case is the container's named home volume (`glasshouse-ci-home-*`) that
# DOES persist across runs, and is the one thing this fixture must never
# touch. Measured: with only `XDG_DATA_HOME` redirected, a dead
# `keyring-XXXXXX/control` socket directory (no credential data, but a
# leftover) still accumulated under the real home on every run.

secret_service_fixture_run() {
    local lib_dir fixture_home status
    lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    fixture_home="$(mktemp -d)"

    XDG_DATA_HOME="$fixture_home" XDG_CACHE_HOME="$fixture_home" dbus-run-session -- \
        "$lib_dir/secret-service-fixture-inner.sh" "$@"
    status=$?

    rm -rf "$fixture_home"
    return "$status"
}
