#!/usr/bin/env python3
"""The busy/idle classifier in `worker-watch.sh`, against real captured panes.

WHY THIS EXISTS
---------------
The watch decides a worker has finished by looking at its pane. Twice on
2026-08-27 it announced "went idle with NO report" for a worker that was
working, and the second time was **because the first was fixed by adding one
string** — a retry countdown — while the pattern's spinner set still did not
list the glyph the pane happened to be drawing.

A false idle is not a harmless notification. Acknowledging one ends the watch,
so nothing is armed for when the worker really does finish (practice §57).

Every sample below is a real capture or a faithful reduction of one. When a
future pane state fools the watch again, add the capture here first.
"""
import re
import subprocess
import sys
from pathlib import Path

WATCH = Path(__file__).resolve().parents[1] / "worker-watch.sh"


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

    # The announcement has to carry the pane's own last line. Both false idles
    # would have been obvious from the notification alone if it had.
    text = WATCH.read_text()
    if text.count("last line was") < 2:
        failures.append(
            "the idle announcements no longer carry the pane's last line; "
            "a notification that quotes the pane is what makes a false idle "
            "visible without opening it"
        )

    if failures:
        print(f"test_worker_watch: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(
        f"test_worker_watch: ok — {len(BUSY)} busy and {len(IDLE)} idle "
        "pane captures classified correctly"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
