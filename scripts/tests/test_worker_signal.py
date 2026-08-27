#!/usr/bin/env python3
"""The Stop hook that tells the orchestrator a worker's turn ended.

WHY THIS EXISTS
---------------
Before it, `worker-watch.sh` inferred "finished" by matching a pattern against
a cmux pane. That pattern was wrong three times on 2026-08-27 — a retry
countdown drew no spinner; a spinner glyph was missing; and the glyphs added to
fix that were the same ones the harness prints in its *completion* line, so two
finished workers went unreported for 35 and 47 minutes.

The harness emits the event already. These tests hold the two properties that
matter and neither is about a glyph: a worker signals, and the orchestrator
never signals itself.
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HOOK = REPO / "scripts" / "hooks" / "worker-turn-ended.sh"
SETTINGS = REPO / ".claude" / "settings.json"


def _main_checkout() -> Path:
    """Where the hook actually writes, which is not where this test runs.

    The hook resolves the MAIN checkout through `git worktree list` on purpose
    (§62): every worktree's marker has to land where the orchestrator's watch
    looks. This test used `parents[2]` — the checkout it was invoked from — so
    it wrote in one place and looked in another, and could only pass in main.
    It failed `lint / script tests` in every worktree until a worker diagnosed
    it. Two places that must agree, and only one of them had been told the rule.
    """
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO), "worktree", "list"],
            capture_output=True, text=True, check=True,
        ).stdout
        return Path(out.splitlines()[0].split()[0])
    except Exception:
        return REPO


DONE = _main_checkout() / ".agent-runtime" / "done"


def fire(project_dir: str) -> None:
    env = dict(os.environ, CLAUDE_PROJECT_DIR=project_dir)
    r = subprocess.run([str(HOOK)], env=env, capture_output=True, text=True, cwd=str(REPO))
    assert r.returncode == 0, (
        f"the hook exited {r.returncode}; a Stop hook that fails costs a worker's "
        f"turn and reports nothing\n{r.stderr}"
    )
    assert not r.stdout.strip(), f"the hook printed to a worker's transcript: {r.stdout!r}"


def main() -> int:
    failures = []
    import json

    watch = (REPO / "scripts" / "worker-watch.sh").read_text()

    # The Stop hook fires at the end of every model TURN, not the end of the
    # work. Treating its marker as authoritative latched "done" on the first
    # turn boundary and announced a worker 42 minutes into its package. The
    # pane must gate: busy always wins, and the marker only labels which
    # signal fired.
    if "if is_busy; then" not in watch:
        failures.append(
            "worker-watch.sh no longer gates on is_busy alone; a done marker that "
            "can override a busy pane turns a turn-end event into a permanent "
            "'finished', which is the 2026-08-27 defect"
        )
    if "worker_signalled && is_busy" in watch:
        failures.append(
            "the done marker is short-circuiting the busy check again — a transient "
            "turn-end event is being stored as durable state"
        )

    settings = json.loads(SETTINGS.read_text())
    if "Stop" not in settings.get("hooks", {}):
        failures.append(
            ".claude/settings.json no longer registers a Stop hook, so no worker "
            "will announce its own turn end and the watch falls back to reading panes"
        )

    # Use a name no real worker has, so a leftover marker can never make this
    # skip the one check it exists for — which it would have done, silently.
    # The fixture directory must really exist and really be inside a git
    # checkout, because the hook locates the main worktree by asking git from
    # it. A subdirectory of this repo satisfies both and resolves to the same
    # main checkout any real worktree would. Its name gives the marker's name:
    # `glasshouse-signal-selftest` -> `signal-selftest`, which no real worker
    # uses, so a leftover marker can never make this skip its own check.
    fixture = REPO / "glasshouse-signal-selftest"
    marker = DONE / "signal-selftest"
    marker.unlink(missing_ok=True)
    fixture.mkdir(exist_ok=True)
    try:
        fire(str(fixture))
    finally:
        fixture.rmdir()
    if True:
        pass
        if not marker.exists():
            failures.append(
                "a worker worktree did not produce a done signal — the hook and this "
                "test must resolve the same main checkout, or the test writes in one "
                "place and looks in another and can only pass in main"
            )
        else:
            body = marker.read_text()
            if "turn ended" not in body:
                failures.append(f"the signal does not say what it is: {body!r}")
            marker.unlink()

    # The orchestrator works in the main checkout. A self-signal would make it
    # announce its own completion and acknowledge work nobody did.
    # The MAIN checkout, not REPO. Run from a worktree, `REPO` is that worktree
    # — so firing the hook at it forged a done marker named after the live
    # worker whose tree the gate happened to be running in, and a watch armed
    # for that worker then announced it finished. The gate was manufacturing
    # the exact false positive the hook was added to remove.
    before = set(p.name for p in DONE.iterdir()) if DONE.is_dir() else set()
    fire(str(_main_checkout()))
    after = set(p.name for p in DONE.iterdir()) if DONE.is_dir() else set()
    if after != before:
        failures.append(f"the main checkout signalled itself: {after - before}")

    with tempfile.TemporaryDirectory() as tmp:
        fire(tmp)
        after2 = set(p.name for p in DONE.iterdir()) if DONE.is_dir() else set()
        if after2 != before:
            failures.append(f"an unrelated directory signalled: {after2 - before}")

    if failures:
        print(f"test_worker_signal: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1
    print("test_worker_signal: ok — a worker signals, the orchestrator does not")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
