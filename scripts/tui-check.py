#!/usr/bin/env python3
"""Regression checks that drive the REAL Glasshouse TUI in a pty this script owns.

The invariant every check here defends: a keystroke that changes the interface
must reach the terminal as a repaint. Unit tests cannot see that — they observe
`ShellState` after `handle_key`, and a shipped freeze proved state is not the
screen: `n` set `Overlay::HarnessChoice` correctly and the run loop never drew
it, and because an open overlay also stops the idle animation the terminal went
silent forever. Every defect found in the TUI so far was of that shape.

How a repaint is made observable, which is the whole trick:

    the idle animation writes ~18 KB/s, so "bytes arrived after the key" is
    true whether or not the key did anything.

So each check first presses `a`, which pauses motion, and waits for the stream
to go quiet. Against that silence a key's repaint is the only thing that can
arrive, and a key that draws nothing produces exactly the shipped failure: a
terminal that never speaks again. The rendered screen is reconstructed from the
bytes as well, so a check asserts on what a user would see, not on byte counts.

Each key runs in its own child. A key that fails to open its overlay leaves the
following ESC to be read by the top-level handler, where ESC means quit — one
child per key keeps that from cascading into every later check.

Usage:
    scripts/tui-check.py                       # target/release/glasshouse
    scripts/tui-check.py --binary $(which glasshouse)
    scripts/tui-check.py --keys n,s --verbose

Exits non-zero if any check fails, so it can join a gate.
"""

from __future__ import annotations

import argparse
import atexit
import codecs
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import threading
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_BINARY = os.path.join(REPO, "target", "release", "glasshouse")

# The keys the shell binds at top level, from shell/state/mod.rs's
# `handle_control_key`. `q`, `t` and `a` are exercised by the base checks
# instead; `N` starts a headless session and is left out of the default set for
# the same reason `n` carries a warning below.
DEFAULT_KEYS = "n,s,o,p,e,f"

# A key that only reads state repaints within one animation tick (~50 ms, the
# gap the idle stream keeps). 250 ms is five ticks: far above every key
# measured on this machine (≤ 5 ms for all but `s`) yet still under the point
# where a user reads the interface as hung. Raise it with --latency-budget to
# gate a build with a known-slow key rather than to hide one.
DEFAULT_LATENCY_BUDGET_MS = 250.0

# The first frame is drawn before the event loop starts, so the binary has only
# to open its stores to speak. Generous because a cold binary pays page-in.
STARTUP_BUDGET_S = 5.0

# A repaint is a positioned run of cells; the smallest real one measured (`f`,
# which redraws only the status and footer) is 106 bytes. Anything at or under
# a bare cursor report is not a frame.
MIN_REPAINT_BYTES = 16

_LIVE: list["Tui"] = []


def _kill_everything() -> None:
    """Cleanup runs on every exit path — a leaked child holds a pty forever."""
    for tui in list(_LIVE):
        tui.close()


atexit.register(_kill_everything)


class Screen:
    """A terminal's visible grid, rebuilt from the bytes the child writes.

    Only what ratatui emits is interpreted: absolute cursor positioning, the
    two erases, and text. Everything else (SGR above all) is skipped, because
    colour is not what these checks assert on. Escape sequences and UTF-8
    characters are buffered across chunk boundaries, or a sequence split by a
    read lands in the grid as literal text.
    """

    _CSI = re.compile(r"\x1b\[([0-9;?]*)([@-~])")
    _CHARSET = re.compile(r"\x1b[()][0-9A-Za-z]")
    _OSC = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")

    def __init__(self, rows: int, cols: int) -> None:
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.row = self.col = 0
        self._pending = ""
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data: bytes) -> None:
        text = self._pending + self._decoder.decode(data)
        self._pending = ""
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b":
                rest = text[i:]
                if len(rest) < 2 and i + 1 >= len(text):
                    self._pending = rest
                    return
                match = self._CSI.match(rest) or self._CHARSET.match(rest) or self._OSC.match(rest)
                if match:
                    if match.re is self._CSI:
                        self._csi(match.group(1), match.group(2))
                    i += match.end()
                    continue
                if self._looks_incomplete(rest):
                    self._pending = rest
                    return
                i += 1
                continue
            if ch == "\r":
                self.col = 0
            elif ch == "\n":
                self.row = min(self.row + 1, self.rows - 1)
            elif ch == "\b":
                self.col = max(self.col - 1, 0)
            elif ch >= " ":
                if self.row < self.rows and self.col < self.cols:
                    self.grid[self.row][self.col] = ch
                self.col += 1
            i += 1

    @staticmethod
    def _looks_incomplete(rest: str) -> bool:
        """True while a sequence could still be completed by the next read."""
        return bool(re.fullmatch(r"\x1b\[?[0-9;?]*", rest) or re.fullmatch(r"\x1b[()\]]?", rest))

    def _csi(self, params: str, final: str) -> None:
        nums = [int(p) for p in params.replace("?", "").split(";") if p.isdigit()]
        if final in "Hf":
            self.row = (nums[0] - 1) if nums else 0
            self.col = (nums[1] - 1) if len(nums) > 1 else 0
        elif final == "J" and (not nums or nums[0] == 2):
            self.grid = [[" "] * self.cols for _ in range(self.rows)]
            self.row = self.col = 0
        elif final == "K":
            if self.row < self.rows:
                for x in range(self.col, self.cols):
                    self.grid[self.row][x] = " "
        elif final == "A":
            self.row = max(0, self.row - (nums[0] if nums else 1))
        elif final == "B":
            self.row = min(self.rows - 1, self.row + (nums[0] if nums else 1))
        elif final == "C":
            self.col = min(self.cols - 1, self.col + (nums[0] if nums else 1))
        elif final == "D":
            self.col = max(0, self.col - (nums[0] if nums else 1))

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.grid)

    def pending_note(self) -> str | None:
        """A status note still promising work, e.g. `checking harness versions…`.

        Glasshouse ends an in-flight note with an ellipsis and replaces it when
        the work lands (`settings ready`). A quiet terminal still showing one
        means the work never arrived — the freeze, wearing a progress message.
        Only the status row is examined; body text truncates with `…` too.
        """
        painted = [line for line in self.text().splitlines() if line.strip()]
        if not painted:
            return None
        status = painted[-1].strip()
        return status if ("\u2026" in status or "..." in status) else None

    def ink(self) -> int:
        """Painted cells — a blank screen is a draw that produced nothing."""
        return sum(1 for row in self.grid for cell in row if cell not in (" ", " "))

    def overlay_title(self) -> str | None:
        """The title of the topmost bordered box, e.g. `┌ settings ───┐`.

        Environment-independent, unlike any particular row of the box: the
        harness picker, settings, overview, project and events overlays all
        draw one, and its absence after ESC is how a close is verified.
        """
        for row in self.grid:
            line = "".join(row)
            # `┌ settings ──────┐`: the title is what sits between the corner
            # and the run of rule that pads it out to the box's width.
            match = re.search(r"┌\s*([^─┌┐\n]{1,48}?)\s*─{2,}", line)
            if match and match.group(1).strip():
                return match.group(1).strip()
        return None


class Tui:
    """The real binary on a real pty, with a reader thread draining it.

    The thread is what makes latency measurable: `select` in the main loop
    would only notice output when the main thread happens to look, and the
    number this reports is the gap between the keystroke and the terminal's
    next byte.
    """

    def __init__(self, binary: str, cwd: str, rows: int, cols: int) -> None:
        self.binary, self.cwd, self.rows, self.cols = binary, cwd, rows, cols
        self.master = -1
        self.proc: subprocess.Popen | None = None
        self.screen = Screen(rows, cols)
        self.raw = bytearray()
        self.last_byte_at = 0.0
        self.eof = False
        self._cv = threading.Condition()

    def start(self) -> None:
        master, slave = pty.openpty()
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.cols, 0, 0))
        env = dict(os.environ)
        # Project rule: an inherited provider base URL is scrubbed for children
        # the harness spawns, never for the caller's own shell.
        for name in ("ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"):
            env.pop(name, None)
        env["TERM"] = env.get("TERM", "xterm-256color")
        env["LINES"], env["COLUMNS"] = str(self.rows), str(self.cols)
        self.proc = subprocess.Popen(
            [self.binary],
            cwd=self.cwd,
            env=env,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            # Its own process group, so a session the TUI started dies with it.
            start_new_session=True,
        )
        os.close(slave)
        self.master = master
        self.last_byte_at = time.monotonic()
        _LIVE.append(self)
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self) -> None:
        while True:
            try:
                ready, _, _ = select.select([self.master], [], [], 0.05)
            except (OSError, ValueError):
                break
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                chunk = b""
            with self._cv:
                if not chunk:
                    self.eof = True
                    self._cv.notify_all()
                    return
                self.raw += chunk
                self.screen.feed(chunk)
                self.last_byte_at = time.monotonic()
                self._cv.notify_all()

    def press(self, data: bytes) -> float:
        """Write a key and return the instant it went in."""
        now = time.monotonic()
        os.write(self.master, data)
        return now

    def wait_for_output(self, since: float, timeout: float) -> float | None:
        """Seconds from `since` to the next byte, or None if none arrives."""
        deadline = since + timeout
        with self._cv:
            while self.last_byte_at <= since and not self.eof:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None
                self._cv.wait(remaining)
            return None if self.last_byte_at <= since else self.last_byte_at - since

    def wait_quiet(self, quiet: float = 0.35, timeout: float = 6.0) -> bool:
        """Wait until nothing has been written for `quiet` seconds."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._cv:
                idle = time.monotonic() - self.last_byte_at
                if idle >= quiet:
                    return True
                self._cv.wait(min(quiet - idle, 0.1))
        return False

    def bytes_written(self) -> int:
        with self._cv:
            return len(self.raw)

    def snapshot(self) -> str:
        with self._cv:
            return self.screen.text()

    def ink(self) -> int:
        with self._cv:
            return self.screen.ink()

    def overlay_title(self) -> str | None:
        with self._cv:
            return self.screen.overlay_title()

    def pending_note(self) -> str | None:
        with self._cv:
            return self.screen.pending_note()

    def contains(self, needle: bytes) -> bool:
        with self._cv:
            return needle in bytes(self.raw)

    def alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    def wait_exit(self, timeout: float) -> int | None:
        assert self.proc is not None
        try:
            return self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None

    def close(self) -> None:
        """Kill the process group and close the pty, however we got here."""
        if self.proc is not None and self.proc.poll() is None:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(os.getpgid(self.proc.pid), sig)
                except (ProcessLookupError, PermissionError):
                    break
                try:
                    self.proc.wait(timeout=2.0)
                    break
                except subprocess.TimeoutExpired:
                    continue
        if self.master >= 0:
            try:
                os.close(self.master)
            except OSError:
                pass
            self.master = -1
        if self in _LIVE:
            _LIVE.remove(self)


class Report:
    def __init__(self) -> None:
        self.rows: list[tuple[str, bool, str, float | None]] = []

    def add(self, name: str, ok: bool, detail: str, ms: float | None = None) -> None:
        self.rows.append((name, ok, detail, ms))
        mark = "PASS" if ok else "FAIL"
        timing = f"  [{ms:.0f} ms]" if ms is not None else ""
        print(f"  {mark}  {name}{timing} — {detail}", flush=True)

    def failed(self) -> int:
        return sum(1 for _, ok, _, _ in self.rows if not ok)

    def table(self) -> None:
        width = max(len(name) for name, _, _, _ in self.rows)
        print("\n" + "=" * (width + 46))
        print(f"{'check'.ljust(width)}  result  latency   detail")
        print("-" * (width + 46))
        for name, ok, detail, ms in self.rows:
            timing = f"{ms:7.0f}ms" if ms is not None else "        —"
            print(f"{name.ljust(width)}  {'PASS  ' if ok else 'FAIL  '} {timing}  {detail[:70]}")
        print("=" * (width + 46))
        bad = self.failed()
        print(f"{len(self.rows) - bad} passed, {bad} failed")


def pause_motion(tui: Tui) -> bool:
    """`a` pauses the idle animation; the checks need the silence it leaves."""
    tui.press(b"a")
    return tui.wait_quiet(quiet=0.35, timeout=6.0)


def base_checks(args, report: Report) -> None:
    """Everything that can be asserted without pressing an overlay key."""
    tui = Tui(args.binary, args.cwd, args.rows, args.cols)
    try:
        start = time.monotonic()
        tui.start()
        first = tui.wait_for_output(start, STARTUP_BUDGET_S)
        if first is None:
            report.add("startup/draws", False, f"silent for {STARTUP_BUDGET_S:.0f}s after exec")
            return
        time.sleep(args.settle)
        ink, painted = tui.ink(), tui.bytes_written()
        report.add(
            "startup/draws",
            ink >= 200 and painted >= 1000,
            f"{painted} bytes, {ink} painted cells",
            first * 1000,
        )
        report.add(
            "startup/full-screen",
            tui.contains(b"\x1b[?1049h"),
            "alternate screen entered" if tui.contains(b"\x1b[?1049h") else "no ?1049h — not full screen",
        )
        # Selection is what a user loses to ?1003h; ?1000h/?1006h keep it.
        any_motion = tui.contains(b"\x1b[?1003h")
        report.add(
            "startup/text-selection-kept",
            not any_motion,
            "any-motion mouse tracking (?1003h) is on — shift-drag selection breaks"
            if any_motion
            else "no ?1003h",
        )
        if args.verbose:
            print("\n--- first frame ---\n" + tui.snapshot() + "\n--- end ---\n")

        quiet = pause_motion(tui)
        report.add(
            "motion/pause-silences",
            quiet,
            "`a` stopped the animation" if quiet else "`a` did not stop the stream in 6s",
        )
        before = tui.bytes_written()
        tui.press(b"a")
        time.sleep(1.5)
        resumed = tui.bytes_written() - before
        report.add(
            "motion/resume-animates",
            resumed >= 1000,
            f"{resumed} bytes in 1.5s after resuming",
        )

        tui.press(b"q")
        rc = tui.wait_exit(3.0)
        left_alt = tui.contains(b"\x1b[?1049l")
        report.add(
            "quit/exits-and-restores",
            rc == 0 and left_alt,
            f"rc={rc}, alternate screen left={left_alt}",
        )
    finally:
        tui.close()


def key_check(args, report: Report, key: str) -> None:
    """One key, one child: press it into silence and assert a frame arrives."""
    data = key.encode()
    tui = Tui(args.binary, args.cwd, args.rows, args.cols)
    try:
        start = time.monotonic()
        tui.start()
        if tui.wait_for_output(start, STARTUP_BUDGET_S) is None:
            report.add(f"key-{key}/repaints", False, "the shell never drew; key not pressed")
            return
        time.sleep(args.settle)
        if not pause_motion(tui):
            report.add(f"key-{key}/repaints", False, "could not silence the animation; key not pressed")
            return

        before_screen, before_bytes = tui.snapshot(), tui.bytes_written()
        pressed = tui.press(data)
        latency = tui.wait_for_output(pressed, args.key_timeout)
        # Reacting is not the same as arriving: `s` answers in a millisecond
        # with "settings: checking harness versions…" and only then builds the
        # overlay. Wait for the interface to stop changing before reading it.
        settled = tui.wait_quiet(quiet=args.quiet, timeout=args.settle_timeout)
        finished_ms = (tui.last_byte_at - pressed) * 1000
        painted = tui.bytes_written() - before_bytes
        after_screen = tui.snapshot()

        if latency is None:
            report.add(
                f"key-{key}/repaints",
                False,
                f"SILENT: no byte in {args.key_timeout:.0f}s — the interface froze",
            )
        elif painted < MIN_REPAINT_BYTES:
            report.add(f"key-{key}/repaints", False, f"only {painted} bytes — not a frame", latency * 1000)
        elif after_screen == before_screen:
            report.add(
                f"key-{key}/repaints",
                False,
                f"{painted} bytes but the visible screen is unchanged",
                latency * 1000,
            )
        else:
            title = tui.overlay_title()
            where = f"opened `{title}`" if title else "redrew"
            report.add(f"key-{key}/repaints", True, f"{where}, {painted} bytes", latency * 1000)

        if latency is not None:
            report.add(
                f"key-{key}/latency",
                latency * 1000 <= args.latency_budget,
                f"first byte, budget {args.latency_budget:.0f} ms",
                latency * 1000,
            )
            title_now = tui.overlay_title()
            note = tui.pending_note()
            if not settled:
                detail = f"still redrawing {args.settle_timeout:.0f}s later — it never came to rest"
            elif note is not None:
                detail = f"quiet, but still promising work: `{note[-60:]}`"
            elif title_now:
                detail = f"stopped changing, showing `{title_now}`"
            else:
                detail = "stopped changing, no overlay"
            report.add(f"key-{key}/settles", settled and note is None, detail, finished_ms if settled else None)

        if args.verbose:
            print(f"\n--- screen after `{key}` ---\n" + after_screen + "\n--- end ---\n")

        if not tui.alive():
            report.add(f"key-{key}/survives", False, f"the shell exited on `{key}` (rc={tui.proc.returncode})")
            return

        # ESC back out. With an overlay open ESC closes it; with none open ESC
        # is the top-level quit, so both outcomes are read against what the
        # key actually put on screen.
        opened = tui.overlay_title()
        esc_at = tui.press(b"\x1b")
        esc_latency = tui.wait_for_output(esc_at, args.key_timeout)
        time.sleep(0.6)
        if opened is None:
            # No overlay to close, so either meaning of top-level ESC is
            # correct — quit, or repaint. Producing neither is the freeze.
            quit_out = not tui.alive()
            report.add(
                f"key-{key}/esc",
                quit_out or esc_latency is not None,
                "no overlay was open; esc quit at top level"
                if quit_out
                else ("esc redrew" if esc_latency is not None else "SILENT: esc neither redrew nor quit"),
                None if esc_latency is None else esc_latency * 1000,
            )
        elif not tui.alive():
            report.add(f"key-{key}/esc", False, f"`{opened}` was open and esc exited the shell instead of closing it")
        else:
            still = tui.overlay_title()
            report.add(
                f"key-{key}/esc",
                still != opened,
                f"`{opened}` closed" if still != opened else f"`{opened}` is still on screen after esc",
                None if esc_latency is None else esc_latency * 1000,
            )
    finally:
        tui.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", default=DEFAULT_BINARY, help="glasshouse binary to drive")
    parser.add_argument("--cwd", default=REPO, help="project the TUI opens (default: this repository)")
    parser.add_argument("--keys", default=DEFAULT_KEYS, help=f"comma-separated keys to check (default {DEFAULT_KEYS})")
    parser.add_argument("--rows", type=int, default=40)
    parser.add_argument("--cols", type=int, default=120)
    parser.add_argument("--settle", type=float, default=1.5, help="seconds to let the first frames finish")
    parser.add_argument("--key-timeout", type=float, default=8.0, help="how long a key may stay silent before FAIL")
    parser.add_argument(
        "--settle-timeout", type=float, default=10.0, help="seconds a key's own redraws may go on before FAIL"
    )
    # Longer than the widest measured gap inside one key's reaction: `s`
    # answers instantly with a note and the settings overlay lands ~1.2 s
    # later, so a shorter window would call the note the final screen.
    parser.add_argument("--quiet", type=float, default=2.0, help="seconds of silence that mean a key is finished")
    parser.add_argument("--latency-budget", type=float, default=DEFAULT_LATENCY_BUDGET_MS, help="ms")
    parser.add_argument("--verbose", action="store_true", help="print the reconstructed screens")
    args = parser.parse_args()

    if not os.path.isfile(args.binary) or not os.access(args.binary, os.X_OK):
        print(f"no executable at {args.binary} — build it first (cargo build --release)", file=sys.stderr)
        return 2

    print(f"binary   {args.binary}")
    print(f"project  {args.cwd}")
    print(f"terminal {args.cols}x{args.rows}\n")

    report = Report()
    base_checks(args, report)
    for key in [k for k in args.keys.split(",") if k]:
        # `n` starts a session outright where exactly one harness is enabled;
        # the child's process group is killed on the way out so nothing it
        # spawned outlives the check.
        key_check(args, report, key)
    report.table()
    return 1 if report.failed() else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        _kill_everything()
