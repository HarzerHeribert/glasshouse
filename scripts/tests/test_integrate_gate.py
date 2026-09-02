#!/usr/bin/env python3
"""`integrate.sh` must compile the library's own test module.

Why this test exists, in one paragraph, because the reason is the whole
point. On 2026-09-02 commit `9f513d9` reached `main` with `cargo check
--tests` broken: `FreePool::adopt_observed` grew a fifth argument and a
four-argument straggler survived inside `routing/session.rs`'s own
`#[cfg(test)]` module. Neither worktree was broken on its own -- the other
package in that integration was authored before the signature changed -- so
the breakage existed only in the merge. It survived the blocking gate
because that gate runs INTEGRATION-test binaries, and those compile the
library WITHOUT `cfg(test)`; nothing in it ever compiled `--lib` with tests
enabled, so the error was invisible to every target it ran.

The fix is one command, and this test is what keeps it there.
"""

import re
import sys
from pathlib import Path

INTEGRATE = Path(__file__).resolve().parents[2] / "scripts" / "integrate.sh"
BLAST = Path(__file__).resolve().parents[2] / "scripts" / "blast-radius.sh"


def _source() -> str:
    return INTEGRATE.read_text(encoding="utf-8")


def _blast_source() -> str:
    return BLAST.read_text(encoding="utf-8")


def test_integrate_checks_the_libs_own_test_module() -> None:
    src = _source()
    assert "cargo check -p glasshouse --tests" in src, (
        "integrate.sh no longer compiles the library's own test module. "
        "No other gate does: integration-test binaries compile the lib "
        "without cfg(test), so a #[cfg(test)] straggler is invisible to them. "
        "See 9f513d9."
    )


def test_the_check_runs_before_the_targeted_gate() -> None:
    """Order matters: a green targeted gate on a tree whose lib tests do not
    compile is a result about nothing, and reporting it first invites the
    integrator to believe it."""
    src = _source()
    check = src.find("cargo check -p glasshouse --tests")
    gate = src.find("blast-radius.sh --targeted")
    assert check != -1 and gate != -1, "one of the two gate steps is missing"
    assert check < gate, (
        "the lib-test compile check must run BEFORE the targeted gate, "
        "so a broken test build fails loudly instead of hiding behind a "
        "green target list"
    )


def test_the_check_is_blocking_not_advisory() -> None:
    """A warning here would have changed nothing on 2026-09-02 -- the
    integrator read a green summary and pushed."""
    src = _source()
    window = src[src.find("cargo check -p glasshouse --tests") :][:800]
    assert re.search(r"\bexit 1\b", window), (
        "the lib-test compile check must abort integrate.sh, not warn. "
        "The failure it guards was pushed to main by an integrator who read "
        "a green summary."
    )


def test_blast_radius_targeted_checks_the_libs_own_test_module() -> None:
    """The same hole existed in `blast-radius.sh --targeted`, which other
    flows call directly without going through `integrate.sh` -- a worker's
    own gate, a fix-forward worker. A green there must mean the same thing
    it means in integrate.sh, so the check lives in both."""
    src = _blast_source()
    assert "cargo check -p glasshouse --tests" in src, (
        "blast-radius.sh --targeted no longer compiles the library's own "
        "test module; every target it runs is an integration-test binary "
        "and none of them compiles the lib with cfg(test). See 9f513d9."
    )


def test_blast_radius_check_runs_before_the_targeted_targets_and_blocks() -> None:
    src = _blast_source()
    check = src.find("cargo check -p glasshouse --tests")
    targets = src.find("--targeted: distance-zero targets only")
    assert check != -1 and targets != -1, "one of the two targeted-gate steps is missing"
    assert check < targets, (
        "the lib-test compile check must run BEFORE --targeted's target list, "
        "so a broken test build fails loudly instead of hiding behind green targets"
    )
    window = src[check:][:900]
    assert re.search(r"\bexit 1\b", window), (
        "the lib-test compile check in blast-radius.sh must abort, not warn"
    )


def main() -> int:
    failures = 0
    for name, fn in sorted(globals().items()):
        if not name.startswith("test_") or not callable(fn):
            continue
        try:
            fn()
        except AssertionError as exc:
            failures += 1
            print(f"FAIL {name}: {exc}")
        else:
            print(f"ok   {name}")
    if failures:
        print(f"\n{failures} failure(s)")
        return 1
    print("\nall integrate-gate tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
