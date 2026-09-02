#!/usr/bin/env python3
"""The busy/idle/never-started classifier in `worker-watch.sh`, against real
captured panes and, for the loop-level behaviour, the real script itself.

WHY THIS EXISTS
---------------
The watch decides a worker has finished by looking at its pane. Twice on
2026-08-27 it announced "went idle with NO report" for a worker that was
working, and the second time was **because the first was fixed by adding one
string** — a retry countdown — while the pattern's spinner set still did not
list the glyph the pane happened to be drawing.

A false idle is not a harmless notification. Acknowledging one ends the watch,
so nothing is armed for when the worker really does finish (practice §57).

On 2026-08-30 the same shape hit the OTHER quiet signal: three workers were
flagged `WORKER NEVER STARTED` roughly 20 times each, right after
`new-worker.sh` had already proven their prompt landed. Every worker's first
action, per the ARM instruction, is a Monitor tool call — and a pane mid-tool-
call shows only a `⎿` line, nothing else. `is_never_started` reused
`last_words_from`'s filter, which strips `⎿` lines on purpose (they are
unreadable quoted as "last words"), so the one thing on screen that proved the
worker had started was thrown away before the never-started check ever saw it.

Every sample below is a real capture or a faithful reduction of one. When a
future pane state fools the watch again, add the capture here first.
"""
import os
import re
import select
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

WATCH = Path(__file__).resolve().parents[1] / "worker-watch.sh"
REPO = WATCH.parent.parent

# worker-watch.sh itself never trusts `parents[...]` for "the main checkout"
# (see its own comment above SCRIPT_DIR/MAIN_COMMON) — it resolves through
# git-common-dir, which is the real main checkout regardless of which
# worktree's copy of the script is running. `REPO` above is just "the tree
# this test file lives in"; from a worker's worktree that is a different
# directory than where the script actually reads and writes its idle/done
# markers, so a test that cleaned up under `REPO/.agent-runtime` left real
# stray fixtures in the main checkout (found: `.agent-runtime/idle/selftest-*`
# never removed after a worktree run). Resolve the marker root the same way.
_MAIN_COMMON = subprocess.run(
    ["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
    check=True, capture_output=True, text=True,
).stdout.strip()
MAIN = Path(_MAIN_COMMON).parent


def _re(name: str) -> str:
    for line in WATCH.read_text().splitlines():
        if line.startswith(f"{name}="):
            return line[len(name) + 1:].strip().strip("'")
    raise AssertionError(f"worker-watch.sh no longer defines {name}")


def is_busy(screen: str) -> bool:
    """The script's own order: a completion line beats every busy signal."""
    if re.search(_re("DONE_RE"), screen):
        return False
    return bool(re.search(_re("BUSY_RE"), screen))


def is_never_started(screen: str) -> bool:
    """Mirrors has_worker_output()/is_never_started() in the real script:
    strip everything that is pure startup boilerplate and see if anything
    is left. Extracted from the script's own STARTUP_ONLY_RE rather than
    hand-duplicated, the same way is_busy() above uses DONE_RE/BUSY_RE — a
    version of worker-watch.sh that has not been fixed yet defines no such
    variable, and _re() fails loudly rather than silently comparing against
    a filter this test invented.
    """
    boilerplate = _re("STARTUP_ONLY_RE")
    remaining = [ln for ln in screen.split("\n") if not re.search(boilerplate, ln)]
    return not any(remaining)


BUSY = {
    "star spinner with an elapsed timer (the first 2026-08-27 miss)":
        "✶ Photosynthesizing… (1h 34m 48s · almost done thinking with high effort)",
    "the other star spinner, token counter":
        "✽ Discombobulating… (11m 7s · ↓ 34.9k tokens)",
    "an API retry countdown, which draws no spinner at all":
        "✻ Waiting for API response · will retry in 2m 8s · check your network",
    "the interrupt hint":
        "  ⏵⏵ auto mode on · esc to interrupt",
    "a braille spinner":
        "⠹ Working… (3m 2s)",
    "an elapsed timer with a glyph nobody has listed yet":
        "◐ Doing something new… (42s · ↓ 1.2k tokens)",
    "a resumed worker mid-turn after a relayed correction (2026-09-02 false nag)":
        "✻ Mulling… (1m 34s · ↓ 8.4k tokens)",
}

IDLE = {
    "the completion line, which uses the SAME glyph as the spinner "
    "(the third 2026-08-27 miss: two finished workers unreported for 35 and 47 minutes)":
        "✻ Churned for 35m 7s · done 8:42 AM\n"
        "❯ \n"
        "  Opus 5 · high  · worker · ~2 · Glasshouse Windows …   5h 10% 4h15m · 7d 63%\n"
        "  ████████▊░░░░░░░░░░░░░░░░╎░░░ 20% · 203k/1M                  47m · ~$9.40",
    "a completion line for a longer run":
        "✽ Worked for 2h 4m 9s · done 11:15 AM\n❯ ",
    "a finished pane, prompt and status bar":
        "⏺ Done. Report written.\n"
        "❯ \n"
        "  Opus 5 · high  · glasshous…/claud…airing        5h 33% 1h03m\n"
        "  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00\n"
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
    "the status bar's own unparenthesised session clock must not read as busy":
        "  Opus 5 · high  · worker            5h 21% 3h01m · 7d 61%\n"
        "  ███████░░░░░░░░░░░░░░░░░░╎░░░ 25% · 254k/1M   1h34m · ~$14.41",
    "a parenthesised hint that is not a timer":
        "  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
    "a bare shell prompt":
        "eneas ➜ ~/projects/glasshouse  main ❯",
}

# The real fixture for the 2026-08-30 defect: only a `⎿` tool-call line above
# the boilerplate. Neither busy (BUSY_RE deliberately excludes a leading `⎿`)
# nor -- before this fix -- distinguishable from true silence.
MID_TOOL_CALL_SCREEN = (
    "Tip: Use /help for more information\n"
    "❯ \n"
    "  ⎿  Running Monitor(description: \"Worker continuity watch\")…\n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)

# The case the signal exists for: nothing but the startup banner.
TRULY_NEVER_STARTED_SCREEN = (
    "Tip: Use /help for more information\n"
    "❯ \n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)

BUSY_SCREEN = (
    "Tip: Use /help for more information\n"
    "❯ \n"
    "  ✻ Flowing… (12s)\n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)

# A quiet-looking pane, but ALREADY started (unlike TRULY_NEVER_STARTED_SCREEN)
# -- used to drive a worker to WORKER DONE rather than WORKER NEVER STARTED.
ALREADY_STARTED_QUIET_SCREEN = (
    "⏺ Done. Report written.\n"
    "❯ \n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)

# The 2026-09-02 capture: a worker resumed after being announced DONE, mid-turn.
RESUMED_BUSY_SCREEN = (
    "Tip: Use /help for more information\n"
    "❯ \n"
    "  ✻ Mulling… (1m 34s · ↓ 8.4k tokens)\n"
    "  Sonnet 5 · high  · worker            5h 21% 3h01m\n"
    "  ░░░░░░░░░░░░░░░░░░░░░░░░╎░░░ 0% · 0/1M            ~$0.00"
)


def _readline_timeout(stream, deadline):
    remaining = deadline - time.time()
    if remaining <= 0:
        return None
    r, _, _ = select.select([stream], [], [], remaining)
    if not r:
        return None
    return stream.readline()


def _wait_for(proc, lines, pattern, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        line = _readline_timeout(proc.stdout, deadline)
        if line is None:
            continue
        if line == "":
            return False  # EOF: process exited
        lines.append(line.rstrip("\n"))
        if re.search(pattern, line):
            return True
    return False


def run_lifecycle_integration():
    """Drives the REAL worker-watch.sh as a subprocess, against a fake `cmux`
    and a fake `sleep` (the loop's only sleep call, so this is safe to shadow
    globally) so a full cold-start -> announce -> retract -> re-quiet cycle
    finishes in a couple of seconds of wall clock instead of minutes.

    Covers acceptance tests 2, 3 and 4: the true alarm still fires on a
    genuinely empty pane, a worker already retracted from NEVER STARTED does
    not get flagged again with no new evidence, and WORKER DONE still fires
    for that same worker once it goes quiet for a second, unrelated reason.
    """
    failures = []
    all_lines = []
    fake_bin = Path(tempfile.mkdtemp(prefix="ww-fakebin-"))
    screen_file = fake_bin / "screen.txt"
    screen_file.write_text(TRULY_NEVER_STARTED_SCREEN)

    (fake_bin / "cmux").write_text(
        "#!/bin/sh\n"
        'if [ "$1" = "read-screen" ]; then cat "{screen}" 2>/dev/null; fi\n'
        "exit 0\n".format(screen=screen_file)
    )
    (fake_bin / "cmux").chmod(0o755)
    # The only sleep in worker-watch.sh's loop is `sleep 20`; shadowing it
    # unconditionally is safe and turns a minutes-long real cycle into one
    # that finishes in well under a second.
    (fake_bin / "sleep").write_text("#!/bin/sh\nexec /bin/sleep 0.05\n")
    (fake_bin / "sleep").chmod(0o755)

    name = f"selftest-{os.getpid()}"
    idle_dir = MAIN / ".agent-runtime" / "idle"
    done_dir = MAIN / ".agent-runtime" / "done"
    marker = idle_dir / name
    done_file = done_dir / name
    report = Path(tempfile.gettempdir()) / f"ww-report-{os.getpid()}.md"

    env = dict(os.environ)
    env["PATH"] = f"{fake_bin}:{env['PATH']}"

    proc = subprocess.Popen(
        ["bash", str(WATCH), name, "test-surface", str(report), "2"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env,
    )
    try:
        # Test 2: the true alarm still fires.
        if not _wait_for(proc, all_lines, "WORKER NEVER STARTED", 10):
            failures.append(
                "WORKER NEVER STARTED did not fire for a genuinely empty pane "
                "(the case the signal exists for)"
            )
            return failures, all_lines

        # The pane starts producing output -- must retract.
        screen_file.write_text(BUSY_SCREEN)
        if not _wait_for(proc, all_lines, "resuming normal watch", 10):
            failures.append(
                "the NEVER STARTED alarm did not retract once the pane started "
                "producing output"
            )
            return failures, all_lines

        # Test 3: the pane goes quiet-and-empty-looking again, with no new
        # evidence the prompt failed a second time. The same alarm must not
        # re-fire; the worker already proved it started.
        screen_file.write_text(TRULY_NEVER_STARTED_SCREEN)
        deadline = time.time() + 6
        saw_done = False
        while time.time() < deadline:
            line = _readline_timeout(proc.stdout, deadline)
            if line is None:
                continue
            if line == "":
                break
            all_lines.append(line.rstrip("\n"))
            if "WORKER NEVER STARTED" in line:
                failures.append(
                    "NEVER STARTED re-fired for a worker already retracted "
                    f"once, with no new evidence: {line.strip()!r}"
                )
                break
            # Test 4: the other signals are unchanged -- a worker gone quiet
            # for an ordinary reason after having started must still be
            # reported, just not as NEVER STARTED.
            if "WORKER DONE" in line:
                saw_done = True
                break
        if not failures and not saw_done:
            failures.append(
                "after retraction, a worker gone quiet again produced neither "
                "signal -- it should have read as WORKER DONE"
            )
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        marker.unlink(missing_ok=True)
        done_file.unlink(missing_ok=True)
        shutil.rmtree(fake_bin, ignore_errors=True)

    return failures, all_lines


def run_done_retraction_integration():
    """Drives the REAL worker-watch.sh through: busy (so ever_started latches
    and the worker is not classified NEVER STARTED) -> quiet twice -> WORKER
    DONE -> resumed busy (the 2026-09-02 capture) -> retraction NOTE and no
    further STILL UNACKNOWLEDGED within two nag periods -> quiet again ->
    WORKER DONE fires a second time.
    """
    failures = []
    all_lines = []
    fake_bin = Path(tempfile.mkdtemp(prefix="ww-fakebin-done-"))
    screen_file = fake_bin / "screen.txt"
    screen_file.write_text(BUSY_SCREEN)

    (fake_bin / "cmux").write_text(
        "#!/bin/sh\n"
        'if [ "$1" = "read-screen" ]; then cat "{screen}" 2>/dev/null; fi\n'
        "exit 0\n".format(screen=screen_file)
    )
    (fake_bin / "cmux").chmod(0o755)
    (fake_bin / "sleep").write_text("#!/bin/sh\nexec /bin/sleep 0.05\n")
    (fake_bin / "sleep").chmod(0o755)

    name = f"selftest-done-{os.getpid()}"
    idle_dir = MAIN / ".agent-runtime" / "idle"
    done_dir = MAIN / ".agent-runtime" / "done"
    marker = idle_dir / name
    done_file = done_dir / name
    report = Path(tempfile.gettempdir()) / f"ww-report-done-{os.getpid()}.md"

    env = dict(os.environ)
    env["PATH"] = f"{fake_bin}:{env['PATH']}"

    # Short nag (1s) so "no further STILL UNACKNOWLEDGED within two nag
    # periods" is a fast, bounded check rather than a long sleep.
    proc = subprocess.Popen(
        ["bash", str(WATCH), name, "test-surface", str(report), "1"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env,
    )
    try:
        # Prove it started (busy), then go quiet-but-already-started twice to
        # reach WORKER DONE rather than WORKER NEVER STARTED.
        screen_file.write_text(ALREADY_STARTED_QUIET_SCREEN)
        if not _wait_for(proc, all_lines, "WORKER DONE", 10):
            failures.append("WORKER DONE did not fire for an already-started, now-quiet worker")
            return failures, all_lines

        # The worker is resumed (a relayed correction) and goes busy again --
        # must retract, not keep nagging STILL UNACKNOWLEDGED.
        screen_file.write_text(RESUMED_BUSY_SCREEN)
        if not _wait_for(proc, all_lines, "is busy again after being announced DONE", 10):
            failures.append(
                "the DONE announcement did not retract once the pane went "
                "busy again after a resume"
            )
            return failures, all_lines

        # No STILL UNACKNOWLEDGED for two nag periods (nag=1s) while busy.
        deadline = time.time() + 2.5
        while time.time() < deadline:
            line = _readline_timeout(proc.stdout, deadline)
            if line is None:
                continue
            if line == "":
                break
            all_lines.append(line.rstrip("\n"))
            if "STILL UNACKNOWLEDGED" in line:
                failures.append(
                    f"STILL UNACKNOWLEDGED fired after retraction while still "
                    f"busy: {line.strip()!r}"
                )
                break

        if not failures:
            # Genuinely quiet again -- WORKER DONE must fire a second time.
            screen_file.write_text(ALREADY_STARTED_QUIET_SCREEN)
            if not _wait_for(proc, all_lines, "WORKER DONE", 10):
                failures.append(
                    "after retraction, a worker gone quiet again did not "
                    "re-announce WORKER DONE"
                )
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        marker.unlink(missing_ok=True)
        done_file.unlink(missing_ok=True)
        shutil.rmtree(fake_bin, ignore_errors=True)

    return failures, all_lines


def main() -> int:
    failures = []

    for why, screen in BUSY.items():
        if not is_busy(screen):
            failures.append(f"should be BUSY but reads as idle — {why}\n    {screen!r}")

    for why, screen in IDLE.items():
        if is_busy(screen):
            hit = re.search(_re("BUSY_RE"), screen)
            failures.append(
                f"should be IDLE but reads as busy — {why}\n"
                f"    matched {hit.group(0) if hit else '?'!r} in {screen!r}"
            )

    # Test 1: the false alarm, reproduced then fixed. A pane mid-tool-call —
    # the state that misfired for three real workers on 2026-08-30 — must not
    # read as never-started. On main, before STARTUP_ONLY_RE existed,
    # is_never_started reused last_words_from's filter (which strips `⎿`
    # lines), so this assertion fails there: _re("STARTUP_ONLY_RE") raises.
    if is_never_started(MID_TOOL_CALL_SCREEN):
        failures.append(
            "should NOT read as never-started — a pane mid-tool-call (only a "
            "`⎿` line on screen) is running, not silent\n"
            f"    {MID_TOOL_CALL_SCREEN!r}"
        )
    # The same fixture is also not BUSY (BUSY_RE deliberately excludes a
    # leading `⎿`) -- this is exactly the gap the defect lived in: neither
    # check recognised it as activity.
    if is_busy(MID_TOOL_CALL_SCREEN):
        failures.append(
            "test fixture assumption broke: a `⎿` tool-call line now reads as "
            "BUSY_RE-busy, so it no longer exercises the never-started gap"
        )

    # Test 2 (classification half; the lifecycle half runs below): the case
    # the signal was written for must still fire.
    if not is_never_started(TRULY_NEVER_STARTED_SCREEN):
        failures.append(
            "should read as never-started — nothing but the startup banner "
            f"is on screen\n    {TRULY_NEVER_STARTED_SCREEN!r}"
        )

    # The announcement has to carry the pane's own last line. Both false idles
    # would have been obvious from the notification alone if it had.
    text = WATCH.read_text()
    if text.count("last line was") < 2:
        failures.append(
            "the idle announcements no longer carry the pane's last line; "
            "a notification that quotes the pane is what makes a false idle "
            "visible without opening it"
        )

    lifecycle_failures, lifecycle_lines = run_lifecycle_integration()
    failures.extend(lifecycle_failures)

    done_failures, done_lines = run_done_retraction_integration()
    failures.extend(done_failures)
    lifecycle_failures = lifecycle_failures + done_failures
    lifecycle_lines = lifecycle_lines + done_lines

    if failures:
        print(f"test_worker_watch: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        if lifecycle_failures:
            print("  lifecycle transcript:", file=sys.stderr)
            for ln in lifecycle_lines:
                print(f"    {ln}", file=sys.stderr)
        return 1

    print(
        f"test_worker_watch: ok — {len(BUSY)} busy and {len(IDLE)} idle pane "
        "captures classified correctly, the mid-tool-call false alarm no "
        "longer fires, the true never-started alarm still fires and does not "
        "repeat after retraction"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
