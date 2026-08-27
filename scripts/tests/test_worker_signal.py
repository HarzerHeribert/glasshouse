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
DONE = REPO / ".agent-runtime" / "done"
SETTINGS = REPO / ".claude" / "settings.json"


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

    settings = json.loads(SETTINGS.read_text())
    if "Stop" not in settings.get("hooks", {}):
        failures.append(
            ".claude/settings.json no longer registers a Stop hook, so no worker "
            "will announce its own turn end and the watch falls back to reading panes"
        )

    marker = DONE / "windows-session"
    existed = marker.exists()
    if not existed:
        fire("/Users/eneas/projects/glasshouse-windows-session")
        if not marker.exists():
            failures.append("a worker worktree did not produce a done signal")
        else:
            body = marker.read_text()
            if "turn ended" not in body:
                failures.append(f"the signal does not say what it is: {body!r}")
            marker.unlink()

    # The orchestrator works in the main checkout. A self-signal would make it
    # announce its own completion and acknowledge work nobody did.
    before = set(p.name for p in DONE.iterdir()) if DONE.is_dir() else set()
    fire(str(REPO))
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
