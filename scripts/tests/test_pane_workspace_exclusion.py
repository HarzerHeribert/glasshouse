"""`pane` must never ride along on a `--workspace` invocation in either gate.

GH-PANE-KICKOFF's own mutation (dropping `--exclude pane` from one line)
SURVIVED every existing check: `pane` is clean, so quietly widening what gets
built and linted produces no failure at all -- the exclusion had no guard.
This is the smallest one: every `--workspace` in scripts/ci-local.sh and
.github/workflows/ci-extended.yml must carry `--exclude pane` on the same
line, so a future line that forgets it is caught here rather than the day
`pane` grows a dependency expensive enough to make the omission visible.
"""

import pathlib

REPO = pathlib.Path(__file__).resolve().parents[2]
CI_LOCAL = REPO / "scripts" / "ci-local.sh"
CI_EXTENDED = REPO / ".github" / "workflows" / "ci-extended.yml"


def _workspace_invocation_lines(path):
    lines = []
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if "--workspace" not in stripped:
            continue
        if stripped.startswith("#"):
            continue
        lines.append(line)
    return lines


def test_ci_local_has_workspace_invocations_to_check():
    """A guard over zero lines proves nothing; fail loudly if the shape changes."""
    assert len(_workspace_invocation_lines(CI_LOCAL)) == 8


def test_every_ci_local_workspace_invocation_excludes_pane():
    offenders = [
        line
        for line in _workspace_invocation_lines(CI_LOCAL)
        if "--exclude pane" not in line
    ]
    assert offenders == [], "missing --exclude pane:\n" + "\n".join(offenders)


def test_ci_extended_has_workspace_invocations_to_check():
    assert len(_workspace_invocation_lines(CI_EXTENDED)) == 6


def test_every_ci_extended_workspace_invocation_excludes_pane():
    offenders = [
        line
        for line in _workspace_invocation_lines(CI_EXTENDED)
        if "--exclude pane" not in line
    ]
    assert offenders == [], "missing --exclude pane:\n" + "\n".join(offenders)


if __name__ == "__main__":
    # Every other file here self-runs under `python3 <file>`, which is how
    # ci-local.sh's "script tests" step invokes them -- a pytest-style file
    # with no entry point is silently never run (found 2026-09-01: ten tests,
    # green for five days, never once executed by the gate).
    import sys

    failed = 0
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except AssertionError as e:
                failed += 1
                print(f"FAIL {name}: {e}")
    sys.exit(1 if failed else 0)
