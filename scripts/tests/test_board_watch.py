#!/usr/bin/env python3
"""board-watch.sh composes worker-watch.sh, prompt-watch.sh, pipeline.sh and
stale-workspaces.sh into ONE digest line per window plus an immediate `!`
line for a small interrupt class (REPORT, PROMPT, QUIET, CI FAILURE, DRY).

WHY THIS EXISTS
---------------
CLAUDE.md arms one persistent Monitor per worker plus one each for prompts,
pipeline dryness and stale panes -- every one of them fires per event, and
Claude Code's Monitor primitive turns each stdout line into its own turn. An
orchestrator supervising several workers and a CI matrix was measured
spending its context triaging one event at a time (GH-BOARD-WATCH packet).
This is the coalescing layer: it must still surface a report or a stuck
prompt the instant it appears (the interrupt class), but everything else
waits for one digest line per window, and prints NOTHING at all when the
board genuinely has not changed -- except a heartbeat, so silence is never
mistaken for a dead watch.

This test drives the REAL board-watch.sh as a subprocess, the same way
test_worker_watch.py drives the real worker-watch.sh: a fake `cmux` and a
fake `gh` on PATH (board-watch.sh's own worker/prompt classification is
worker-watch.sh's and prompt-watch.sh's real code, run against synthetic
panes), a fake `sleep` so a multi-window run finishes in about a second of
wall clock, and small fixture stand-ins for pipeline.sh/stale-workspaces.sh
(passed in via BOARD_WATCH_PIPELINE_SH / BOARD_WATCH_STALE_WORKSPACES_SH,
env overrides board-watch.sh defines for exactly this) so the test does not
depend on this checkout's real live worktrees or gh runs holding still.
"""
import os
import select
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

WATCH = Path(__file__).resolve().parents[1] / "board-watch.sh"
WORKER_WATCH = WATCH.parent / "worker-watch.sh"
PROMPT_WATCH = WATCH.parent / "prompt-watch.sh"
REPO = WATCH.parent.parent


def _readline_timeout(stream, deadline):
    remaining = deadline - time.time()
    if remaining <= 0:
        return None
    r, _, _ = select.select([stream], [], [], remaining)
    if not r:
        return None
    return stream.readline()


def _drain_until(proc, pattern_prefix, timeout):
    """Read lines until one starts with pattern_prefix, or timeout. Returns
    (matched: bool, lines: list[str]) -- every line read, in order, matched
    one included."""
    lines = []
    deadline = time.time() + timeout
    while time.time() < deadline:
        line = _readline_timeout(proc.stdout, deadline)
        if line is None:
            continue
        if line == "":
            break
        line = line.rstrip("\n")
        lines.append(line)
        if line.startswith(pattern_prefix):
            return True, lines
    return False, lines


def _drain_for(proc, seconds):
    lines = []
    deadline = time.time() + seconds
    while time.time() < deadline:
        line = _readline_timeout(proc.stdout, deadline)
        if line is None:
            continue
        if line == "":
            break
        lines.append(line.rstrip("\n"))
    return lines


class Fixtures:
    """One fake `cmux`, one fake `gh`, one fake instant `sleep`, and static
    fixture stand-ins for pipeline.sh/stale-workspaces.sh, all rooted under
    one temp directory so a test can steer every composed script's answer
    without touching real board state.
    """

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="bw-fixtures-"))
        self.bin = self.dir / "bin"
        self.bin.mkdir()
        self.state = self.dir / "state"
        self.state.mkdir()

        (self.bin / "cmux").write_text(
            "#!/bin/sh\n"
            'dir="$BW_FIXTURE_STATE"\n'
            'if [ "$1 $2" = "read-screen --surface" ]; then\n'
            '  key=$(printf %s "$3" | tr ":" "_")\n'
            '  cat "$dir/surface_$key" 2>/dev/null\n'
            'elif [ "$1 $2" = "read-screen --workspace" ]; then\n'
            '  key=$(printf %s "$3" | tr ":" "_")\n'
            '  cat "$dir/workspace_$key" 2>/dev/null\n'
            'elif [ "$1 $2" = "workspace list" ]; then\n'
            '  cat "$dir/workspace_list" 2>/dev/null\n'
            'fi\n'
            'exit 0\n'
        )
        (self.bin / "cmux").chmod(0o755)

        (self.bin / "gh").write_text(
            "#!/bin/sh\n"
            'dir="$BW_FIXTURE_STATE"\n'
            'if [ "$1 $2" = "run view" ]; then\n'
            '  cat "$dir/ci_$3.json" 2>/dev/null\n'
            'fi\n'
            'exit 0\n'
        )
        (self.bin / "gh").chmod(0o755)

        # The loop's only sleep call, unconditionally shadowed to run in
        # ~50ms regardless of the (integer, seconds) TICK it was given --
        # the same trick test_worker_watch.py uses for its own lifecycle
        # test, for the same reason: board-watch.sh's TICK/WINDOW arithmetic
        # is bash integer math, so the seconds argument must stay a real
        # integer for `--tick 1 --window N` to mean anything, while the
        # actual wall-clock cost of each tick should not.
        (self.bin / "sleep").write_text("#!/bin/sh\nexec /bin/sleep 0.05\n")
        (self.bin / "sleep").chmod(0o755)

        # Static pipeline.sh stand-in: always live=5 against a floor of 2,
        # i.e. never dry, in every test here -- DRY is not one of the two
        # interrupt kinds the acceptance tests require, and letting it vary
        # would make the digest change for a reason no test is watching for.
        (self.dir / "pipeline.sh").write_text(
            "#!/bin/sh\n"
            "printf 'pipeline  live=5  waiting=0  ready-to-dispatch=0  (floor 2)\\n'\n"
            "exit 0\n"
        )
        (self.dir / "pipeline.sh").chmod(0o755)

        # Static stale-workspaces.sh stand-in: always "none". Used by every
        # test except the six-changes one, which points BOARD_WATCH_STALE_
        # WORKSPACES_SH at stale_cycle.sh instead (see cycling_stale_sh).
        (self.dir / "stale_static.sh").write_text(
            "#!/bin/sh\necho 'stale-workspaces: none'\n"
        )
        (self.dir / "stale_static.sh").chmod(0o755)

        # A count that increases by one on every invocation (persisted in a
        # file under state/), rendered as that many STALE lines mod 7 --
        # gives collect_stale a genuinely different reading almost every
        # tick without the test having to race a background writer against
        # the running board-watch.sh subprocess.
        (self.dir / "stale_cycle.sh").write_text(
            "#!/bin/sh\n"
            'f="$BW_FIXTURE_STATE/stale_calls"\n'
            'n=$(cat "$f" 2>/dev/null || echo 0)\n'
            'n=$((n + 1))\n'
            'echo "$n" > "$f"\n'
            'm=$((n % 7))\n'
            'i=1\n'
            'while [ "$i" -le "$m" ]; do\n'
            '  echo "STALE workspace:$i fake-$i — close it: true"\n'
            '  i=$((i + 1))\n'
            'done\n'
            '[ "$m" -eq 0 ] && echo "stale-workspaces: none"\n'
            'exit 0\n'
        )
        (self.dir / "stale_cycle.sh").chmod(0o755)

    def env(self, extra=None):
        env = dict(os.environ)
        env["PATH"] = f"{self.bin}:{env['PATH']}"
        env["BW_FIXTURE_STATE"] = str(self.state)
        env["BOARD_WATCH_PIPELINE_SH"] = str(self.dir / "pipeline.sh")
        env["BOARD_WATCH_STALE_WORKSPACES_SH"] = str(self.dir / "stale_static.sh")
        # worker-watch.sh and prompt-watch.sh are the REAL scripts under
        # test, pinned explicitly rather than left to board-watch.sh's own
        # $REPO resolution: $REPO resolves to the MAIN checkout via
        # git-common-dir, whose copies do not carry whatever is currently
        # uncommitted in this worktree (this is exactly what hung the first
        # draft of board-watch.sh in manual testing -- see its header for
        # prompt-watch.sh specifically).
        env["BOARD_WATCH_WORKER_WATCH_SH"] = str(WORKER_WATCH)
        env["BOARD_WATCH_PROMPT_WATCH_SH"] = str(PROMPT_WATCH)
        if extra:
            env.update(extra)
        return env

    def set_screen(self, surface: str, text: str):
        key = surface.replace(":", "_")
        (self.state / f"surface_{key}").write_text(text)

    def set_workspace_screen(self, ws: str, text: str):
        key = ws.replace(":", "_")
        (self.state / f"workspace_{key}").write_text(text)

    def set_workspace_list(self, refs):
        (self.state / "workspace_list").write_text(
            "\n".join(f"* {r} some-pane" for r in refs) + "\n"
        )

    def cleanup(self):
        shutil.rmtree(self.dir, ignore_errors=True)


QUIET_SCREEN = (
    "Tip: Use /help for more information\n"
    "❯ \n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)

PROMPT_SCREEN = (
    "  │ Bash: grep -r foo .                                              │\n"
    "  │                                                                   │\n"
    "  Do you want to proceed?\n"
    "  ❯ 1. Yes\n"
    "    2. No\n"
)


def start(fixtures: Fixtures, args, extra_env=None):
    return subprocess.Popen(
        ["bash", str(WATCH), *args],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        env=fixtures.env(extra_env),
    )


def test_once_flag(failures):
    fixtures = Fixtures()
    try:
        fixtures.set_screen("surface:0", QUIET_SCREEN)
        proc = subprocess.run(
            ["bash", str(WATCH), "--once", "--worker",
             "solo:surface:0:/nonexistent-report.md"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            env=fixtures.env(), timeout=15,
        )
        lines = [ln for ln in proc.stdout.splitlines() if ln.strip()]
        if proc.returncode != 0:
            failures.append(f"--once exited {proc.returncode}, expected 0: {proc.stdout!r}")
        board_lines = [ln for ln in lines if ln.startswith("BOARD ")]
        if len(board_lines) != 1:
            failures.append(f"--once should print exactly one BOARD line, got {lines!r}")
    finally:
        fixtures.cleanup()


def test_report_interrupts_same_tick(failures):
    fixtures = Fixtures()
    try:
        fixtures.set_screen("surface:0", QUIET_SCREEN)
        fixtures.set_workspace_list([])
        report = Path(tempfile.gettempdir()) / f"bw-report-{os.getpid()}.md"
        report.unlink(missing_ok=True)
        proc = start(fixtures, [
            "--tick", "1", "--window", "100", "--heartbeat", "1000",
            "--worker", f"solo:surface:0:{report}",
        ])
        try:
            # Give priming (the first, silent observation) time to run
            # against a report that does not exist yet.
            time.sleep(0.3)
            report.write_text("done")
            matched, lines = _drain_until(proc, "! ", timeout=10)
            if not matched or "REPORT" not in lines[-1] or "solo" not in lines[-1]:
                failures.append(f"expected a '! ... REPORT solo ...' line, got {lines!r}")
        finally:
            proc.terminate()
            proc.wait(timeout=5)
            report.unlink(missing_ok=True)
    finally:
        fixtures.cleanup()


def test_prompt_interrupts_before_digest(failures):
    """Acceptance test 2: a prompt screen produces `! PROMPT` the same tick
    it appears, and it precedes the digest line that same window boundary
    closes -- not the other way round, and not merely eventually.
    """
    fixtures = Fixtures()
    try:
        fixtures.set_screen("surface:0", QUIET_SCREEN)
        fixtures.set_workspace_list([])
        proc = start(fixtures, [
            "--tick", "1", "--window", "1", "--heartbeat", "1000",
            "--worker", "solo:surface:0:/nonexistent-report.md",
        ])
        try:
            time.sleep(0.3)  # let priming (no prompt yet) complete
            fixtures.set_workspace_list(["workspace:5"])
            fixtures.set_workspace_screen("workspace:5", PROMPT_SCREEN)
            matched, lines = _drain_until(proc, "BOARD ", timeout=10)
            if not matched:
                failures.append(f"no BOARD line ever printed: {lines!r}")
                return
            prompt_idx = next(
                (i for i, ln in enumerate(lines) if ln.startswith("! ") and "PROMPT" in ln),
                None,
            )
            board_idx = next(i for i, ln in enumerate(lines) if ln.startswith("BOARD "))
            if prompt_idx is None:
                failures.append(f"no '! ... PROMPT ...' line before the digest: {lines!r}")
            elif prompt_idx > board_idx:
                failures.append(f"PROMPT line came AFTER the digest, not before: {lines!r}")
            elif "workspace:5" not in lines[prompt_idx]:
                failures.append(f"PROMPT line did not name the stuck pane: {lines[prompt_idx]!r}")
        finally:
            proc.terminate()
            proc.wait(timeout=5)
    finally:
        fixtures.cleanup()


def test_six_changes_one_digest(failures):
    """Acceptance test 1: many state changes across one window still close
    it with exactly one digest line, not one per change.
    """
    fixtures = Fixtures()
    try:
        fixtures.set_screen("surface:0", QUIET_SCREEN)
        fixtures.set_workspace_list([])
        proc = start(fixtures, [
            "--tick", "1", "--window", "8", "--heartbeat", "1000",
            "--worker", "solo:surface:0:/nonexistent-report.md",
        ], extra_env={"BOARD_WATCH_STALE_WORKSPACES_SH": str(fixtures.dir / "stale_cycle.sh")})
        try:
            matched, lines = _drain_until(proc, "BOARD ", timeout=10)
            if not matched:
                failures.append(f"no BOARD line printed despite a changing board: {lines!r}")
                return
            board_lines = [ln for ln in lines if ln.startswith("BOARD ")]
            if len(board_lines) != 1:
                failures.append(
                    f"expected exactly one digest for the window, got {len(board_lines)}: {lines!r}"
                )
        finally:
            proc.terminate()
            proc.wait(timeout=5)
    finally:
        fixtures.cleanup()


def test_heartbeat_and_silence(failures):
    """Acceptance test 3: a static board prints nothing for several windows,
    then exactly one heartbeat line at the configured window count.
    """
    fixtures = Fixtures()
    try:
        fixtures.set_screen("surface:0", QUIET_SCREEN)
        fixtures.set_workspace_list([])
        proc = start(fixtures, [
            "--tick", "1", "--window", "1", "--heartbeat", "10",
            "--worker", "solo:surface:0:/nonexistent-report.md",
        ])
        try:
            # Three windows' worth of wall clock (fake sleep ~50ms/tick):
            # nothing should print at all.
            early = _drain_for(proc, 0.4)
            noisy = [ln for ln in early if ln.strip()]
            if noisy:
                failures.append(f"static board printed before the heartbeat window: {noisy!r}")
            # Run out to and past the 10th window; exactly one heartbeat.
            matched, rest = _drain_until(proc, "BOARD ", timeout=10)
            if not matched:
                failures.append("no heartbeat ever printed for a static board")
            else:
                board_lines = [ln for ln in rest if ln.startswith("BOARD ")]
                if len(board_lines) != 1:
                    failures.append(f"expected exactly one heartbeat, got {board_lines!r}")
        finally:
            proc.terminate()
            proc.wait(timeout=5)
    finally:
        fixtures.cleanup()


def main() -> int:
    if not WATCH.exists():
        print(f"test_board_watch: {WATCH} does not exist", file=sys.stderr)
        return 1
    if not os.access(WATCH, os.X_OK):
        print(f"test_board_watch: {WATCH} is not executable", file=sys.stderr)
        return 1

    tests = [
        ("once_flag_exits_zero", test_once_flag),
        ("report_interrupts_same_tick", test_report_interrupts_same_tick),
        ("prompt_interrupts_before_digest", test_prompt_interrupts_before_digest),
        ("six_changes_one_digest", test_six_changes_one_digest),
        ("heartbeat_and_silence", test_heartbeat_and_silence),
    ]

    total_failures = []
    passed = 0
    for name, fn in tests:
        failures = []
        try:
            fn(failures)
        except Exception as exc:  # a broken fixture should fail loudly, not hang the suite
            failures.append(f"{name} raised {exc!r}")
        if failures:
            for f in failures:
                total_failures.append(f"{name}: {f}")
        else:
            passed += 1

    if total_failures:
        print(f"test_board_watch: {len(total_failures)} failure(s)", file=sys.stderr)
        for f in total_failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(f"test_board_watch: {passed} passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
